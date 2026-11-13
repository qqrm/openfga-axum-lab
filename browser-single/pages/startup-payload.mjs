const startupFiles = ['guest.sh', 'entrypoint.sh', 'runtime/openfga', 'runtime/bootstrap'];
const bootstrapSourceCommit = '64d6fb4614b6674284a32cee2f4f84179dfb2a2f';
const sha256Pattern = /^[0-9a-f]{64}$/;

// One reviewed native CLI supplies initial provisioning and its opt-in deny
// Check. The archived rootfs CLI remains the probe and acceptance authority.
export function validateBootstrapBuild(build, entry) {
  if (!build || typeof build !== 'object' || Array.isArray(build) ||
      build.schema_version !== 1 || build.source_commit !== bootstrapSourceCommit ||
      build.target !== 'linux/amd64' || !/^sha256:[0-9a-f]{64}$/.test(build.image_id ?? '') ||
      build.timeout_scope !== 'bootstrap_model_post_and_get' ||
      build.default_model_timeout_seconds !== 5 || build.min_model_timeout_seconds !== 5 ||
      build.max_model_timeout_seconds !== 30 || build.browser_model_timeout_seconds !== 30 ||
      build.empty_store_check !== 'one_real_higher_consistency_ownership_deny' ||
      build.empty_store_check_default !== false || build.browser_empty_store_check !== true ||
      build.empty_store_check_timeout_seconds !== 5 || build.empty_store_check_tests !== 'passed' ||
      build.native_tests !== 'passed' ||
      !sha256Pattern.test(build.dockerfile_sha256 ?? '') ||
      !sha256Pattern.test(build.cargo_lock_sha256 ?? '') ||
      !sha256Pattern.test(build.bootstrap_source_sha256 ?? '') ||
      !sha256Pattern.test(build.bootstrap_tests_sha256 ?? '') ||
      !sha256Pattern.test(build.native_test_log_sha256 ?? '') ||
      !Number.isSafeInteger(build.binary_bytes) || build.binary_bytes <= 0 ||
      !sha256Pattern.test(build.binary_sha256 ?? '') ||
      build.binary_bytes !== entry?.bytes || build.binary_sha256 !== entry?.sha256) {
    throw new Error('Unexpected bootstrap build provenance');
  }
}

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
    if (!Number.isSafeInteger(entry?.bytes) || entry.bytes <= 0 || !sha256Pattern.test(entry.sha256 ?? '')) {
      throw new Error(`Invalid startup checksum: ${file}`);
    }
  }
  validateBootstrapBuild(manifest.bootstrap_build, manifest.files['runtime/bootstrap']);
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
