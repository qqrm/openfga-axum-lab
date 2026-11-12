const startupFiles = ['guest.sh', 'entrypoint.sh', 'runtime/openfga'];

export async function loadStartupPayload(base, fetcher = fetch) {
  const response = await fetcher(new URL('runtime/startup-manifest.json', base));
  if (!response.ok) throw new Error(`Cannot load startup manifest: ${response.status}`);
  const manifest = await response.json();
  if (!/^[0-9a-f]{40}$/.test(manifest.base_source_commit ?? '') ||
      !manifest.openfga_build || typeof manifest.openfga_build !== 'object' ||
      Array.isArray(manifest.openfga_build) ||
      JSON.stringify(Object.keys(manifest.files ?? {}).sort()) !== JSON.stringify([...startupFiles].sort())) {
    throw new Error('Unexpected startup manifest');
  }
  for (const file of startupFiles) {
    const entry = manifest.files[file];
    if (!Number.isSafeInteger(entry?.bytes) || entry.bytes <= 0 || !/^[0-9a-f]{64}$/.test(entry.sha256 ?? '')) {
      throw new Error(`Invalid startup checksum: ${file}`);
    }
  }
  const payload = await Promise.all(startupFiles.map(async file => {
    const response = await fetcher(new URL(file, base));
    if (!response.ok) throw new Error(`Cannot load ${file}: ${response.status}`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    const expected = manifest.files[file];
    if (bytes.byteLength !== expected.bytes) throw new Error(`Startup size mismatch: ${file}`);
    const digest = await crypto.subtle.digest('SHA-256', bytes);
    const sha256 = [...new Uint8Array(digest)].map(byte=>byte.toString(16).padStart(2,'0')).join('');
    if (sha256 !== expected.sha256) throw new Error(`Startup checksum mismatch: ${file}`);
    return {name:file.split('/').at(-1),bytes};
  }));
  return {manifest,payload};
}
