import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {loadStartupManifest, modelSha256, runtimeSourceCommit, validateStartupManifest, verifyStartupBytes} from './pages/startup-payload.mjs';

const record = bytes => ({bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex')});
function fixture() {
  const model = record(readFileSync(new URL('./model.json', import.meta.url)));
  const binary = record(Buffer.from('Unit fixture: no guest binary is executed'));
  const proof = record(Buffer.from('Unit fixture provenance'));
  return {
    kind: 'browser_concept_startup', schemaVersion: 1,
    base_source_commit: '99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58', runtimeSourceCommit,
    conceptSourceCommit: 'b'.repeat(40), model, downloadBytes: 650 * 1024**2,
    files: {'guest.sh': proof, 'entrypoint.sh': proof, 'runtime/concept': binary, 'model.json': model,
      'runtime/openfga': {bytes: 63074488, sha256: 'd779834ae225c95502b957efc96a6153937bb2f5f2a1cef9f134847ae8c0d02b'}},
    conceptBuild: {schemaVersion: 1, kind: 'browser_concept_native', sourceCommit: 'b'.repeat(40), sourceTree: 'c'.repeat(40),
      target: 'x86_64-unknown-linux-gnu', binary, model,
      checks: {rustfmt: {passed: true}, clippy: {passed: true}, unitTests: {passed: true}}},
    nativeProof: {passed: true, record: proof},
    retained: {materialization: proof, emulatorOverlay: proof,
      openfgaBuild: {source_commit: '293893b22a53d8a130a5f7c97cab4bc8e91bcaa4', upstream_version: '1.10.3',
        binary_sha256: 'd779834ae225c95502b957efc96a6153937bb2f5f2a1cef9f134847ae8c0d02b',
        authorization_request_timeout_default_seconds: 3, schema_reachability_tests: 'passed'},
      emulatorBuild: {idle_gc_policy: 'pending_local_finalizers_timed_idle_wait', idle_gc_poll_ms: 20,
        idle_gc_tests: 'passed', idle_gc_bridge_tests: 'passed', memory_barrier_tests: 'passed', i32_multiply_tests: 'passed',
        files: {'out.js': {bytes: 279363, sha256: '95f7e6b59d98cd7267e705aa0b2cd670970b984d13c7e81b4d4f677cd65534d2'},
          'qemu-system-x86_64.wasm': {bytes: 41250530, sha256: 'bca14ba86cff602c979e3ae93df80611b186ecbe101542ec14e6809c528e0db3'}}}},
  };
}

test('metadata preview binds the tiny model/native proof and downloads no executable', async () => {
  const manifest = fixture(), requested = [];
  assert.equal(manifest.model.sha256, modelSha256);
  assert.deepEqual(await loadStartupManifest('https://example.test/concept/launcher.js', async url => {
    requested.push(url.pathname); return Response.json(manifest);
  }), manifest);
  assert.deepEqual(requested, ['/concept/runtime/startup-manifest.json']);
});

test('unreviewed sources, changed binaries/model and failed native checks cannot enable startup', () => {
  for (const change of [
    value => { value.runtimeSourceCommit = 'd'.repeat(40); },
    value => { value.model = {...value.model, sha256: 'd'.repeat(64)}; },
    value => { value.conceptBuild.binary = {...value.conceptBuild.binary, bytes: 7}; },
    value => { value.conceptBuild.model = {...value.model, bytes: 7}; },
    value => { value.conceptBuild.sourceCommit = 'd'.repeat(40); },
    value => { value.conceptBuild.target = 'aarch64-unknown-linux-gnu'; },
    value => { value.conceptBuild.checks.clippy.passed = false; },
    value => { value.nativeProof.passed = false; },
    value => { value.retained.openfgaBuild.authorization_request_timeout_default_seconds = 30; },
    value => { value.retained.emulatorBuild.files['out.js'].sha256 = 'd'.repeat(64); },
    value => { delete value.files['runtime/concept']; },
    value => { value.files['runtime/bootstrap'] = value.nativeProof.record; },
  ]) {
    const manifest = fixture(); change(manifest);
    assert.throws(() => validateStartupManifest(manifest));
  }
});

test('payload verifier rejects truncation and same-length substitution before returning bytes', async () => {
  const actual = new TextEncoder().encode('reviewed startup bytes');
  const expected = record(actual);
  assert.deepEqual(await verifyStartupBytes('guest.sh', actual, expected), actual);
  await assert.rejects(verifyStartupBytes('guest.sh', actual.subarray(1), expected), /size mismatch/);
  const substituted = actual.slice(); substituted[0] ^= 1;
  await assert.rejects(verifyStartupBytes('guest.sh', substituted, expected), /checksum mismatch/);
});

test('missing manifest and duplicate identity keys fail before executable downloads', async () => {
  await assert.rejects(loadStartupManifest('https://example.test/concept/launcher.js', async () => new Response('', {status: 404})), /HTTP 404/);
  await assert.rejects(loadStartupManifest('https://example.test/concept/launcher.js', async () =>
    new Response('{"kind":"browser_concept_startup","kind":"other"}')), /Duplicate/);
});
