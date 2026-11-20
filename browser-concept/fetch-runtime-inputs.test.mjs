import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createWriteStream, promises as fs} from 'node:fs';
import {createRequire} from 'node:module';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {pipeline} from 'node:stream/promises';
import test from 'node:test';
import {ApiHttpError, RuntimeInputReader, crc32, extract, fileRecord, openArchive, storageLocation} from './fetch-runtime-inputs.mjs';

const require = createRequire(path.resolve('e2e/package.json'));
const {yazl} = require(path.join(path.dirname(require.resolve('playwright-core/package.json')), 'lib/utilsBundle.js'));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const head = 'a'.repeat(40);
const blob = Buffer.from('exact original archive bytes');
const blobRecord = {bytes: blob.length, sha256: digest(blob)};
async function temporary(t) {
  const directory = await fs.mkdtemp(path.join(tmpdir(), 'concept-input-test-'));
  t.after(() => fs.rm(directory, {recursive: true, force: true}));
  return directory;
}
async function zip(filename, entries) {
  const output = new yazl.ZipFile();
  const completed = pipeline(output.outputStream, createWriteStream(filename, {flags: 'wx'}));
  for (const [name, bytes, options] of entries) output.addBuffer(Buffer.from(bytes), name, options);
  output.end();
  await completed;
}
function metadata(expired = false) {
  return {id: 101, name: 'original-fixture', workflow_run: {id: 11, head_sha: head},
    size_in_bytes: blob.length, digest: 'sha256:' + blobRecord.sha256, expired};
}

test('CRC32 matches the standard vector and incremental chunks', () => {
  assert.equal(crc32(Buffer.from('123456789')), 0xcbf43926);
  assert.equal(crc32(Buffer.from('56789'), crc32(Buffer.from('1234'))), 0xcbf43926);
  assert.equal(crc32(Buffer.alloc(0)), 0);
});

for (const [name, mode, corruption, expected] of [
  ['live input skips the mirror', 'live'],
  ['expired input recovers exact original bytes', 'expired'],
  ['404 input recovers exact original bytes', 404],
  ['410 input recovers exact original bytes', 410],
  ['wrong original digest cannot use recovery', 'wrong', null, /identity mismatch/],
  ['403 is not missing or expired', 403, null, /outside the permitted/],
  ['500 is not missing or expired', 500, null, /outside the permitted/],
  ['network errors cannot use recovery', 'network', null, /network failure/],
  ['corrupt inner archive is rejected', 'expired', 'bytes', /inner ZIP differs/],
  ['wrong producer run is rejected', 'expired', 'producer', /provenance differs/],
  ['wrong producer head is rejected', 'expired', 'head', /producer commit differs/],
  ['explicit null producer head cannot use the fallback field', 'expired', 'null-head', /producer commit differs/],
  ['invalid UTF-8 in producer proof is rejected', 'expired', 'encoding', /encoded data.*valid/],
  ['wrong retained role is rejected', 'expired', 'role', /artifact role differs/],
  ['ambiguous retained member is rejected', 'expired', 'duplicate', /exactly one retained member/],
]) {
  test(name, async t => {
    const root = await temporary(t);
    const reader = new RuntimeInputReader({root, log() {}});
    let mirrors = 0, downloads = 0;
    const originalMetadata = metadata(mode === 'expired');
    if (mode === 'wrong') originalMetadata.digest = 'sha256:' + '0'.repeat(64);
    reader.api = async () => {
      if (typeof mode === 'number') throw new ApiHttpError(mode);
      if (mode === 'network') throw new Error('metadata network failure');
      return originalMetadata;
    };
    reader.downloadArchive = async (aid, destination, bytes, sha) => {
      downloads++;
      assert.deepEqual([aid, bytes, sha], [101, blob.length, blobRecord.sha256]);
      await fs.writeFile(destination, blob, {flag: 'wx'});
      return blobRecord;
    };
    const original = {artifact: 101, run: corruption === 'producer' ? 12 : 11,
      head: corruption === 'head' ? 'b'.repeat(40) : head, label: corruption === 'role' ? 'kernel' : 'retained',
      artifact_name: 'original-fixture', archive: blobRecord};
    if (corruption === 'null-head') { original.head = null; original.workflowCommit = head; }
    original.note = 'utf8-fixture';
    const proofBytes = Buffer.from(JSON.stringify(original));
    if (corruption === 'encoding') proofBytes[proofBytes.indexOf('utf8-fixture')] = 255;
    const filename = path.join(root, 'fixture-mirror.zip');
    const entries = [['retained.zip', corruption === 'bytes' ? Buffer.from('wrong') : blob],
      ['retained-archive-provenance.json', proofBytes]];
    if (corruption === 'duplicate') entries.push(['nested/retained.zip', blob]);
    await zip(filename, entries);
    reader.retainedMirror = async () => { mirrors++; return [filename, {archive: await fileRecord(filename)}]; };
    const operation = () => reader.obtainArchive('retained', 11, head, 101, 'original-fixture', blob.length, blobRecord.sha256);
    if (expected) await assert.rejects(operation, expected);
    else {
      const [destination, bytes, recovery] = await operation();
      assert.deepEqual(await fs.readFile(destination), blob);
      assert.equal(bytes, blob.length);
      if (mode === 'live') assert.equal(recovery, null);
      else {
        assert.equal(recovery.kind, 'verified_original_zip_from_retained_mirror');
        assert.equal(recovery.originalArtifact, 101);
        assert.equal(recovery.reason, mode === 'expired' ? 'original_artifact_expired' : 'original_artifact_metadata_http_' + mode);
      }
    }
    const recoverable = mode === 'expired' || mode === 404 || mode === 410;
    assert.equal(mirrors, recoverable ? 1 : 0);
    assert.equal(downloads, mode === 'live' ? 1 : 0);
  });
}

for (const [name, mutate, expected] of [
  ['exact same-run native producer is accepted', () => {}, null],
  ['failed native producer is rejected', (_metadata, job) => { job.conclusion = 'failure'; }, /did not complete successfully/],
  ['wrong producer source is rejected', (_metadata, job) => { job.head_sha = 'b'.repeat(40); }, /did not complete successfully/],
  ['wrong producer run is rejected', (_metadata, job) => { job.run_id = 12; }, /did not complete successfully/],
  ['producer rerun is rejected', (_metadata, job) => { job.run_attempt = 2; }, /did not complete successfully/],
  ['expired current artifact is rejected', metadata => { metadata.expired = true; }, /has expired/],
  ['oversized current artifact is rejected', metadata => { metadata.size_in_bytes = 1001; }, /size budget/],
  ['wrong current artifact digest is rejected', metadata => { metadata.digest = 'sha256:' + 'b'.repeat(64); }, /identity mismatch/],
  ['wrong current artifact run is rejected', metadata => { metadata.workflow_run.id = 12; }, /identity mismatch/],
  ['ambiguous producer job is rejected', (_metadata, _job, jobs) => { jobs.jobs.push({...jobs.jobs[0]}); jobs.total_count++; }, /exactly one current artifact producer/],
  ['oversized producer job page is rejected', (_metadata, _job, jobs) => { jobs.total_count = 101; }, /bounded|exceeded budget/],
]) {
  test(name, async t => {
    const root = await temporary(t);
    const reader = new RuntimeInputReader({root, env: {GITHUB_SHA: head, ARTIFACT_ID: '101', ARTIFACT_DIGEST: blobRecord.sha256}, log() {}});
    reader.currentRun = 11;
    reader.jobTiming = {id: 200, name: 'Browser concept (chromium)', run_id: 11, head_sha: head, run_attempt: 1,
      source: 'GitHub Actions current job API', started_at: '2026-11-20T08:00:00Z'};
    const artifact = metadata();
    const job = {id: 100, name: 'Build browser concept binary', run_id: 11, head_sha: head,
      status: 'completed', conclusion: 'success', run_attempt: 1};
    const jobs = {total_count: 1, jobs: [job]};
    mutate(artifact, job, jobs);
    reader.api = async suffix => suffix.includes('/artifacts/') ? artifact : jobs;
    let downloads = 0;
    reader.downloadArchive = async () => { downloads++; return blobRecord; };
    const operation = () => reader.currentArtifact('concept-native', 'ARTIFACT_ID', 'ARTIFACT_DIGEST',
      'original-fixture', 'Build browser concept binary', 1000);
    if (expected) { await assert.rejects(operation, expected); assert.equal(downloads, 0); }
    else {
      const [, proof] = await operation();
      assert.equal(downloads, 1);
      assert.deepEqual(proof.producer.job, job);
      assert.deepEqual(proof.consumerJob, reader.jobTiming);
      assert.notEqual(proof.producer.job.id, proof.consumerJob.id);
    }
  });
}

test('fresh Firefox job metadata records its own identity and start', async t => {
  const root = await temporary(t);
  const env = {BROWSER_CONCEPT_ENGINE: 'firefox', BROWSER_DIAGNOSTIC_JOB_NAME: 'Browser concept (firefox)',
    BROWSER_DIAGNOSTIC_VCPUS: '2', GITHUB_RUN_ID: '11', GITHUB_RUN_ATTEMPT: '1', GITHUB_SHA: head};
  const reader = new RuntimeInputReader({root, env, log() {}});
  reader.api = async suffix => suffix.includes('/jobs?') ? {total_count: 1, jobs: [{id: 200, name: env.BROWSER_DIAGNOSTIC_JOB_NAME,
    run_id: 11, head_sha: head, status: 'in_progress', started_at: '2026-11-20T08:30:00Z'}]} :
    {id: 11, head_sha: head, head_branch: 'feat/browser-concept-20261120', run_attempt: 1, event: 'push',
      path: '.github/workflows/browser-concept.yml', status: 'in_progress'};
  await reader.observeCurrentJob('--bundle');
  const timing = JSON.parse(await fs.readFile(path.join(root, 'job-timing.json'), 'utf8'));
  assert.equal(timing.id, 200);
  assert.equal(timing.name, 'Browser concept (firefox)');
  assert.equal(timing.started_at, '2026-11-20T08:30:00Z');
  assert.equal(timing.run_id, 11);
  assert.equal(timing.run_attempt, 1);
  assert.equal(timing.source, 'GitHub Actions current job API');
});

test('stream extraction keeps deflated files and permitted empty logs', async t => {
  const root = await temporary(t);
  const filename = path.join(root, 'input.zip');
  await zip(filename, [['binary', blob, {mode: 0o100555}], ['compiler.log', Buffer.alloc(0)]]);
  const archive = await openArchive(filename);
  try {
    assert.deepEqual(await extract(archive, 'binary', path.join(root, 'binary'), blob.length), blobRecord);
    assert.deepEqual(await extract(archive, 'compiler.log', path.join(root, 'compiler.log'), 0, true), {bytes: 0, sha256: digest(Buffer.alloc(0))});
    await assert.rejects(() => extract(archive, 'compiler.log', path.join(root, 'nonempty-required'), 1), /Invalid selected/);
    await assert.rejects(() => extract(archive, 'binary', path.join(root, 'too-small'), blob.length - 1), /Invalid selected/);
    await assert.rejects(() => extract(archive, 'binary', path.join(root, 'binary'), blob.length), /EEXIST/);
  } finally { archive.close(); }
});

test('selected symlinks and special files are rejected', async t => {
  const root = await temporary(t);
  const filename = path.join(root, 'input.zip');
  await zip(filename, [['symlink', 'target', {mode: 0o120777}], ['fifo', 'x', {mode: 0o010644}]]);
  const archive = await openArchive(filename);
  try {
    for (const name of ['symlink', 'fifo']) await assert.rejects(() => extract(archive, name, path.join(root, name), 100), /Invalid selected/);
  } finally { archive.close(); }
});

for (const [name, mutate, pattern, opening] of [
  ['CRC corruption', bytes => { bytes[30 + bytes.readUInt16LE(26) + bytes.readUInt16LE(28)] ^= 1; }, /CRC mismatch/, false],
  ['local filename drift', bytes => { bytes[30] ^= 1; }, /Local ZIP header differs/, false],
  ['unsafe central backslash', bytes => { const at = bytes.indexOf(Buffer.from([0x50,0x4b,0x01,0x02])); bytes[at + 47] = 92; }, /invalid characters|Unsafe ZIP/, true],
  ['unsafe central NUL', bytes => { const at = bytes.indexOf(Buffer.from([0x50,0x4b,0x01,0x02])); bytes[at + 47] = 0; }, /Unsafe ZIP/, true],
  ['unsafe traversal', bytes => { const at = bytes.indexOf(Buffer.from([0x50,0x4b,0x01,0x02])); Buffer.from('../').copy(bytes, at + 46); }, /invalid relative path|Unsafe ZIP/, true],
  ['invalid UTF-8 filename', bytes => { const at = bytes.indexOf(Buffer.from([0x50,0x4b,0x01,0x02])); bytes.writeUInt16LE(bytes.readUInt16LE(at + 8) | 0x800, at + 8); bytes[at + 46] = 255; }, /encoded data.*valid/, true],
]) {
  test(name + ' is rejected', async t => {
    const root = await temporary(t);
    const filename = path.join(root, 'input.zip');
    await zip(filename, [['abc', 'value', {compress: false}]]);
    const bytes = await fs.readFile(filename);
    mutate(bytes);
    await fs.writeFile(filename, bytes);
    if (opening) await assert.rejects(() => openArchive(filename), pattern);
    else {
      const archive = await openArchive(filename);
      try { await assert.rejects(() => extract(archive, 'abc', path.join(root, 'out'), 100), pattern); }
      finally { archive.close(); }
    }
  });
}

test('duplicate central member names are rejected', async t => {
  const root = await temporary(t);
  const filename = path.join(root, 'input.zip');
  await zip(filename, [['same', 'first'], ['same', 'second']]);
  await assert.rejects(() => openArchive(filename), /member count\/duplicates/);
});

test('only the signed storage allowlist is accepted', () => {
  for (const location of ['http://a.blob.core.windows.net/x', 'https://a.blob.core.windows.net:444/x',
    'https://token@a.blob.core.windows.net/x', 'https://a.blob.core.windows.net.attacker.test/x', 'https://example.org/x']) {
    assert.throws(() => storageLocation(location), /Unexpected artifact storage host/);
  }
  assert.equal(storageLocation('https://a.blob.core.windows.net/x?sig=secret'), 'https://a.blob.core.windows.net/x?sig=secret');
});

test('download binds bytes and sends the token only to the GitHub API', async t => {
  const root = await temporary(t);
  const calls = [];
  const reader = new RuntimeInputReader({root, env: {ARTIFACT_READ_TOKEN: 'test-token'}, log() {},
    fetchImpl: async (url, options) => {
      calls.push({url, options});
      return calls.length === 1 ? new Response(null, {status: 302, headers: {location: 'https://fixture.blob.core.windows.net/archive'}}) :
        new Response(blob, {status: 200});
    }});
  assert.deepEqual(await reader.downloadArchive(101, path.join(root, 'archive.zip'), blob.length, blobRecord.sha256), blobRecord);
  assert.equal(calls.length, 2);
  assert.equal(calls[0].options.headers.Authorization, 'Bearer test-token');
  assert.equal(calls[1].options.headers, undefined);
  assert(calls.every(call => call.options.redirect === 'manual'));
});

for (const [name, response, expectedBytes, sha, pattern] of [
  ['storage redirect', () => new Response(null, {status: 302, headers: {location: 'https://example.org/'}}), blob.length, blobRecord.sha256, /storage network failure/],
  ['archive size overflow', () => new Response(blob), blob.length - 1, blobRecord.sha256, /exceeded pinned size/],
  ['truncated archive', () => new Response(blob.subarray(1)), blob.length, blobRecord.sha256, /ZIP SHA\/size mismatch/],
  ['archive hash mismatch', () => new Response(blob), blob.length, '0'.repeat(64), /ZIP SHA\/size mismatch/],
]) {
  test(name + ' fails without following or retrying', async t => {
    const root = await temporary(t);
    let calls = 0;
    const reader = new RuntimeInputReader({root, env: {ARTIFACT_READ_TOKEN: 'test-token'}, log() {},
      fetchImpl: async () => ++calls === 1 ? new Response(null, {status: 302, headers: {location: 'https://fixture.actions.githubusercontent.com/archive'}}) : response()});
    await assert.rejects(() => reader.downloadArchive(101, path.join(root, 'archive.zip'), expectedBytes, sha), pattern);
    assert.equal(calls, 2);
  });
}

test('metadata has a one MiB body bound and API redirects are not followed', async () => {
  const reader = new RuntimeInputReader({env: {ARTIFACT_READ_TOKEN: 'test-token'}, log() {},
    fetchImpl: async () => new Response(Buffer.alloc(1024 * 1024 + 1, 32))});
  await assert.rejects(() => reader.api('/actions/runs/11'), /Metadata response exceeded budget/);
  reader.fetch = async () => new Response(null, {status: 302, headers: {location: 'https://example.org/'}});
  await assert.rejects(() => reader.api('/actions/runs/11'), error => error instanceof ApiHttpError && error.status === 302);
});

test('invalid UTF-8 in JSON metadata is rejected instead of replaced', async () => {
  const body = Buffer.concat([Buffer.from('{"note":"'), Buffer.from([0xc3, 0x28]), Buffer.from('"}')]);
  const reader = new RuntimeInputReader({env: {ARTIFACT_READ_TOKEN: 'test-token'}, log() {},
    fetchImpl: async () => new Response(body)});
  await assert.rejects(() => reader.api('/actions/runs/11'), /GitHub metadata request failed/);
});
