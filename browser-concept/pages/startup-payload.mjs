import {parseStrictJson} from './protocol.mjs';

export const modelSha256 = '8d82f5df2d5744902d6725f9bff05bc94b64382764359bdc3998d5e0010bd5dd';
export const runtimeSourceCommit = '444b069daf16e9606a0bbf1ea0aea7f4d61b97d8';
const baseSourceCommit = '99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58';
const openfgaSha256 = 'd779834ae225c95502b957efc96a6153937bb2f5f2a1cef9f134847ae8c0d02b';
const emulatorFiles = {
  'out.js': {bytes: 279363, sha256: '95f7e6b59d98cd7267e705aa0b2cd670970b984d13c7e81b4d4f677cd65534d2'},
  'qemu-system-x86_64.wasm': {bytes: 41250530, sha256: 'bca14ba86cff602c979e3ae93df80611b186ecbe101542ec14e6809c528e0db3'},
};
export const startupFiles = Object.freeze(['guest.sh', 'entrypoint.sh', 'runtime/openfga', 'runtime/concept', 'model.json']);
const sha256Pattern = /^[0-9a-f]{64}$/;
const commitPattern = /^[0-9a-f]{40}$/;

function require(condition, message) {
  if (!condition) throw new Error(message);
}

function object(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function validRecord(value) {
  return object(value) && Number.isSafeInteger(value.bytes) && value.bytes > 0 &&
    value.bytes <= 128 * 1024 * 1024 && sha256Pattern.test(value.sha256 ?? '');
}

function sameRecord(left, right) {
  return validRecord(left) && validRecord(right) && left.bytes === right.bytes && left.sha256 === right.sha256;
}

export function validateStartupManifest(manifest) {
  require(object(manifest) && manifest.kind === 'browser_concept_startup' && manifest.schemaVersion === 1,
    'Unexpected concept startup manifest');
  require(manifest.base_source_commit === baseSourceCommit && manifest.runtimeSourceCommit === runtimeSourceCommit &&
    commitPattern.test(manifest.conceptSourceCommit ?? ''), 'Unreviewed runtime source identity');
  require(object(manifest.files) && JSON.stringify(Object.keys(manifest.files).sort()) === JSON.stringify([...startupFiles].sort()),
    'Unexpected concept startup files');
  for (const file of startupFiles) require(validRecord(manifest.files[file]), 'Invalid startup checksum: ' + file);
  require(validRecord(manifest.model) && manifest.model.sha256 === modelSha256 &&
    sameRecord(manifest.model, manifest.files['model.json']), 'The declared policy differs from this concept');
  require(manifest.files['runtime/openfga'].sha256 === openfgaSha256, 'Unexpected OpenFGA binary');
  const build = manifest.conceptBuild;
  require(object(build) && build.kind === 'browser_concept_native' && build.schemaVersion === 1 &&
    build.sourceCommit === manifest.conceptSourceCommit && commitPattern.test(build.sourceTree ?? '') &&
    build.target === 'x86_64-unknown-linux-gnu' && sameRecord(build.binary, manifest.files['runtime/concept']) &&
    sameRecord(build.model, manifest.model), 'Concept binary/model build provenance differs');
  for (const check of ['rustfmt', 'clippy', 'unitTests']) {
    require(build.checks?.[check]?.passed === true, 'Required native source check did not pass: ' + check);
  }
  require(manifest.nativeProof?.passed === true && validRecord(manifest.nativeProof.record),
    'Missing successful native integration evidence');
  const retained = manifest.retained;
  require(object(retained) && validRecord(retained.materialization) && validRecord(retained.emulatorOverlay),
    'Missing retained runtime provenance');
  const fga = retained.openfgaBuild;
  require(object(fga) && fga.source_commit === '293893b22a53d8a130a5f7c97cab4bc8e91bcaa4' &&
    fga.upstream_version === '1.10.3' && fga.binary_sha256 === openfgaSha256 &&
    fga.authorization_request_timeout_default_seconds === 3 && fga.schema_reachability_tests === 'passed',
  'Retained OpenFGA provenance differs');
  const emulator = retained.emulatorBuild;
  require(object(emulator) && emulator.idle_gc_policy === 'pending_local_finalizers_timed_idle_wait' &&
    emulator.idle_gc_poll_ms === 20 && emulator.idle_gc_tests === 'passed' && emulator.idle_gc_bridge_tests === 'passed' &&
    emulator.memory_barrier_tests === 'passed' && emulator.i32_multiply_tests === 'passed',
  'Retained emulator provenance differs');
  for (const [name, record] of Object.entries(emulatorFiles)) {
    require(sameRecord(emulator.files?.[name], record), 'Retained emulator bytes differ: ' + name);
  }
  require(Number.isSafeInteger(manifest.downloadBytes) && manifest.downloadBytes > 0 && manifest.downloadBytes <= 2 * 1024**3,
    'Missing or invalid runtime download size');
  return manifest;
}

export async function loadStartupManifest(base, fetcher = fetch) {
  const response = await fetcher(new URL('runtime/startup-manifest.json', base), {cache: 'no-store'});
  require(response.ok, 'Cannot load the prepared runtime manifest: HTTP ' + response.status);
  const text = await response.text();
  require(new TextEncoder().encode(text).byteLength <= 1024 * 1024, 'Startup manifest is too large');
  return validateStartupManifest(parseStrictJson(text));
}

export async function verifyStartupBytes(name, bytes, expected) {
  require(bytes instanceof Uint8Array && validRecord(expected), 'Invalid startup payload: ' + name);
  require(bytes.byteLength === expected.bytes, 'Startup size mismatch: ' + name);
  const hash = await crypto.subtle.digest('SHA-256', bytes);
  const actual = [...new Uint8Array(hash)].map(byte => byte.toString(16).padStart(2, '0')).join('');
  require(actual === expected.sha256, 'Startup checksum mismatch: ' + name);
  return bytes;
}

export async function loadStartupPayload(base, fetcher = fetch) {
  // Fetch again after opt-in so a metadata preview never authorizes stale or
  // changed executable/model inputs. No executable is loaded by the preview.
  const manifest = await loadStartupManifest(base, fetcher);
  const payload = await Promise.all(startupFiles.map(async name => {
    const response = await fetcher(new URL(name, base), {cache: 'no-store'});
    require(response.ok, 'Cannot load ' + name + ': HTTP ' + response.status);
    const length = response.headers.get('content-length');
    if (length !== null) require(/^\d+$/.test(length) && Number(length) === manifest.files[name].bytes, 'Startup length mismatch: ' + name);
    const bytes = new Uint8Array(await response.arrayBuffer());
    await verifyStartupBytes(name, bytes, manifest.files[name]);
    return {name: name.split('/').at(-1), bytes};
  }));
  return {manifest, payload};
}
