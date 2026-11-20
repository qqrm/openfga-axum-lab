#!/usr/bin/env node
// Restore exact, independently proven producer bytes; never start a VM or build.
import {createHash} from 'node:crypto';
import {createReadStream, createWriteStream, promises as fs} from 'node:fs';
import {createRequire} from 'node:module';
import path from 'node:path';
import {Readable, Transform} from 'node:stream';
import {pipeline} from 'node:stream/promises';
import {setTimeout as delay} from 'node:timers/promises';
import {fileURLToPath} from 'node:url';
import {isDeepStrictEqual} from 'node:util';

export const M = 1024 ** 2;
const REPO = 'qqrm/openfga-axum-lab';
const equal = isDeepStrictEqual;
const positiveInteger = value => Number.isSafeInteger(value) && value > 0;
const requireValue = (value, message) => { if (!value) throw new Error(message); };
const pick = (value, keys) => Object.fromEntries(keys.map(key => [key, value[key]]));
const sumSizes = entries => entries.reduce((sum, entry) => sum + entry.uncompressedSize, 0);
const isDirectory = entry => entry.fileName.endsWith('/');
const sameNames = (actual, expected) => equal([...actual].sort(), [...expected].sort());

export const native = {
  run: 43500949883,
  head: 'bb8284f17e78667c6ea46a2de02de5dc3f3507af',
  job: 132146260409,
  bootstrap: {artifact: 14074756932, name: 'browser-bootstrap-openfga10-paths', bytes: 3321954,
    sha256: '584367cfa6d93ab98593ac9cc939695030b23dcc5634d5da762bee8455926917'},
  evidence: {artifact: 14075336387, name: 'browser-openfga10-paths-native-evidence', bytes: 24005932,
    sha256: '7049ffbd41ba8a33f7f300fcf6c65ab3741145ba08a087b0c0e607b615203526'},
  binaries: {
    openfga: {bytes: 63074488, sha256: 'd779834ae225c95502b957efc96a6153937bb2f5f2a1cef9f134847ae8c0d02b'},
    bootstrap: {bytes: 7444640, sha256: 'e1143b7d5f8cc564e09d12c8f685bd7cf33bd0beb1505f2b7cfc1f6f53e1d275'},
  },
};
const lightweight = {run: 43470193142, head: '0a8d7dc4ba611fe7071b34005410f4dc832d7b71',
  artifact: 14060071538, name: 'browser-log-transport-candidate', bytes: 46909,
  sha256: '2845492a3a08a09f4fc957b7907b920362cf521d96545e5569afc9918182cb34'};
export const specs = [
  ['lightweight', lightweight.run, lightweight.head, lightweight.artifact, lightweight.name,
    lightweight.bytes, lightweight.sha256, 'success', 'push'],
  ['retained', 43402635950, '379e3280c2a1c789071292af141648eb2fc3ffbc', 14029511154,
    'pages-browser-vm-evidence', 260830150, '73e4c4d2f2204ef4da0450e472b50ffb507e52b6a46fe55e0cab153b4fa3976e', 'failure', 'pull_request'],
  ['kernel', 43473084506, '84af5832aed519e7f0628e36afd8d2fc3e4cc270', 14061334425,
    'browser-virtio-rng-kernel-candidate', 3689588, 'c71c1e7fdb705e1b5351ed82d119322c0ff7ff9f9f068d5a75a611aded2935ad', 'success', 'push'],
  ['native', native.run, native.head, native.evidence.artifact, native.evidence.name,
    native.evidence.bytes, native.evidence.sha256, 'success', 'push'],
  ['bootstrap', native.run, native.head, native.bootstrap.artifact, native.bootstrap.name,
    native.bootstrap.bytes, native.bootstrap.sha256, 'success', 'push'],
];
export const MIRROR = {run: 44663922895, head: 'd4794e07c8e9cefef6271f498c6c309ce52b661c',
  job: 136048513540, artifact: 14597878044, name: 'pr36-vcpu-reproduction-inputs', bytes: 308249993,
  sha256: '9d21628183e1d211558959446bbb71d76ef3fcbe21d221cc8f9c4fc51edbaa20'};
export const EMULATOR = {artifact: 14276419843, bytes: 16286946, name: 'pr36-idle-gc-emulator',
  sha256: '6fcd65f16a8a05207b7a15460ae2097ecafa00ee031fcc4c60bda3edf5e1ea30',
  run: 43969702163, head: 'a12b8799fed16e6efc93ab55498e8f165fb1a139', job: 133658307719};

function verifyPins() {
  requireValue(positiveInteger(native.run) && /^[0-9a-f]{40}$/.test(native.head) &&
    [native.bootstrap, native.evidence].every(item => positiveInteger(item.artifact) &&
      positiveInteger(item.bytes) && item.bytes <= 192 * M && /^[0-9a-f]{64}$/.test(item.sha256)),
  'Combined native OpenFGA/bootstrap artifact pins are not verified yet');
  requireValue(positiveInteger(native.job), 'The exact completed native job pin is missing');
  for (const role of ['openfga', 'bootstrap']) {
    const record = native.binaries[role];
    requireValue(record && positiveInteger(record.bytes) && record.bytes <= 128 * M &&
      /^[0-9a-f]{64}$/.test(record.sha256), 'The actual successfully verified native binary pin is missing: ' + role);
  }
  requireValue(native.bootstrap.artifact !== native.evidence.artifact, 'The two named native artifacts must be distinct');
  requireValue(positiveInteger(lightweight.run) && positiveInteger(lightweight.artifact) &&
    positiveInteger(lightweight.bytes) && lightweight.bytes <= 16 * M && /^[0-9a-f]{64}$/.test(lightweight.sha256),
  'Successful exact-source lightweight transport artifact pins are not verified yet');
}

// Keep CRC validation explicitly: yauzl validates sizes, but does not verify the
// decompressed payload CRC. This incremental implementation also supports Node 20.
const crcTable = Uint32Array.from({length: 256}, (_, value) => {
  for (let bit = 0; bit < 8; bit++) value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
  return value >>> 0;
});
export function crc32(bytes, previous = 0) {
  let crc = (previous ^ 0xffffffff) >>> 0;
  for (const byte of bytes) crc = crcTable[(crc ^ byte) & 255] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function zipBackend() {
  const require = createRequire(path.resolve('e2e/package.json'));
  const packagePath = require.resolve('playwright-core/package.json');
  requireValue(require(packagePath).version === '1.65.0', 'The locked Playwright ZIP dependency differs');
  // In locked Playwright 1.65.0 the ZIP exports live here, not in zipBundle.js.
  const {yauzl} = require(path.join(path.dirname(packagePath), 'lib/utilsBundle.js'));
  requireValue(typeof yauzl?.open === 'function', 'The locked ZIP reader is unavailable');
  return yauzl;
}

export async function openArchive(filename) {
  const zip = await new Promise((resolve, reject) => zipBackend().open(filename,
    {autoClose: false, lazyEntries: true, decodeStrings: true, strictFileNames: true, validateEntrySizes: true},
    (error, value) => error ? reject(error) : resolve(value)));
  const archive = {zip, entries: [], byName: new Map(), error: null, close: () => zip.close()};
  zip.on('error', error => { archive.error = error; });
  const centralDirectoryOffset = zip.readEntryCursor;
  try {
    requireValue(Number.isSafeInteger(zip.entryCount) && zip.entryCount <= 10000,
      'Invalid ZIP member count/duplicates');
    await new Promise((resolve, reject) => {
      zip.once('error', reject);
      zip.once('end', resolve);
      zip.on('entry', entry => {
        try {
          const name = entry.fileName;
          if (entry.generalPurposeBitFlag & 0x800) new TextDecoder('utf-8', {fatal: true}).decode(entry.fileNameRaw);
          requireValue(typeof name === 'string' && !name.startsWith('/') && !name.includes('\\') &&
            !name.includes('\0') && !name.split('/').includes('..'), 'Unsafe ZIP member name');
          requireValue(!archive.byName.has(name) && archive.entries.length < 10000,
            'Invalid ZIP member count/duplicates');
          requireValue([entry.uncompressedSize, entry.compressedSize, entry.relativeOffsetOfLocalHeader]
            .every(value => Number.isSafeInteger(value) && value >= 0), 'Invalid ZIP member size/offset');
          archive.entries.push(entry);
          archive.byName.set(name, entry);
          zip.readEntry();
        } catch (error) { zip.close(); reject(error); }
      });
      zip.readEntry();
    });
    // Preserve the previous extractor's rejection of overlapping member data.
    const sorted = [...archive.entries].sort((a, b) => a.relativeOffsetOfLocalHeader - b.relativeOffsetOfLocalHeader);
    for (let i = 0; i < sorted.length; i++) {
      sorted[i].endOffset = sorted[i + 1]?.relativeOffsetOfLocalHeader ?? centralDirectoryOffset;
    }
    return archive;
  } catch (error) { zip.close(); throw error; }
}

function getEntry(archive, name) {
  requireValue(!archive.error, 'ZIP reader failed');
  const entry = archive.byName.get(name);
  requireValue(entry, 'Missing selected ZIP member: ' + name);
  return entry;
}

export async function extract(archive, name, destination, limit, allowEmpty = false) {
  const entry = getEntry(archive, name);
  const type = (entry.externalFileAttributes >>> 16) & 0xf000;
  requireValue(!isDirectory(entry) && (type === 0 || type === 0x8000) &&
    (allowEmpty ? 0 : 1) <= entry.uncompressedSize && entry.uncompressedSize <= limit,
  'Invalid selected ZIP member: ' + name);
  requireValue(!entry.isEncrypted(), 'Encrypted selected ZIP member: ' + name);
  const local = await new Promise((resolve, reject) => archive.zip.readLocalFileHeader(entry,
    (error, value) => error ? reject(error) : resolve(value)));
  requireValue(local.fileName.equals(entry.fileNameRaw) &&
    local.compressionMethod === entry.compressionMethod && local.generalPurposeBitFlag === entry.generalPurposeBitFlag,
  'Local ZIP header differs from central directory: ' + name);
  requireValue(local.fileDataStart + entry.compressedSize <= entry.endOffset,
    'Overlapped selected ZIP member: ' + name);
  await fs.mkdir(path.dirname(destination), {recursive: true});
  const input = await new Promise((resolve, reject) => archive.zip.openReadStream(entry,
    (error, stream) => error ? reject(error) : resolve(stream)));
  let bytes = 0, crc = 0;
  const digest = createHash('sha256');
  const checked = new Transform({transform(block, _encoding, callback) {
    bytes += block.length;
    if (bytes > entry.uncompressedSize || bytes > limit) return callback(new Error('Member read exceeded budget'));
    digest.update(block); crc = crc32(block, crc); callback(null, block);
  }});
  await pipeline(input, checked, createWriteStream(destination, {flags: 'wx'}));
  requireValue(bytes === entry.uncompressedSize, 'Truncated selected ZIP member');
  requireValue(crc === entry.crc32, 'Selected ZIP member CRC mismatch: ' + name);
  return {bytes, sha256: digest.digest('hex')};
}

export function uniqueRetainedMember(archive, basename) {
  const matches = archive.entries.filter(entry => !isDirectory(entry) && path.posix.basename(entry.fileName) === basename);
  requireValue(matches.length === 1, 'Expected exactly one retained member: ' + basename);
  return matches[0].fileName;
}

export function verifyArtifact(metadata, aid, name, runId, head, bytes, sha) {
  requireValue(metadata.id === aid && metadata.name === name && metadata.workflow_run?.id === runId &&
    metadata.workflow_run?.head_sha === head && metadata.size_in_bytes === bytes && metadata.digest === 'sha256:' + sha,
  'Pinned artifact identity mismatch');
  requireValue(typeof metadata.expired === 'boolean', 'Malformed original artifact expiry flag');
}

export function storageLocation(location) {
  let target;
  try { target = new URL(location); } catch { throw new Error('Unexpected artifact storage host'); }
  requireValue(target.protocol === 'https:' && ['', '443'].includes(target.port) && !target.username && !target.password &&
    (target.hostname.endsWith('.blob.core.windows.net') || target.hostname.endsWith('.actions.githubusercontent.com')),
  'Unexpected artifact storage host');
  return target.href;
}

export class ApiHttpError extends Error {
  constructor(status) { super('GitHub metadata HTTP ' + status); this.status = status; }
}

export async function fileRecord(filename) {
  let bytes = 0;
  const digest = createHash('sha256');
  for await (const block of createReadStream(filename)) { bytes += block.length; digest.update(block); }
  return {bytes, sha256: digest.digest('hex')};
}
const readJson = async filename => JSON.parse(new TextDecoder('utf-8', {fatal: true, ignoreBOM: true})
  .decode(await fs.readFile(filename)));
const writeJson = async (filename, value) => {
  await fs.mkdir(path.dirname(filename), {recursive: true});
  await fs.writeFile(filename, JSON.stringify(value, null, 2) + '\n', {flag: 'wx'});
};

export class RuntimeInputReader {
  constructor({env = process.env, root = 'prepared', sourceRoot = 'source-candidate', fetchImpl = fetch,
    log = value => console.log(JSON.stringify(value))} = {}) {
    this.env = env;
    this.root = root;
    this.sourceRoot = sourceRoot;
    this.fetch = fetchImpl;
    this.log = log;
    this.mirrorProof = null;
  }

  request(suffix, timeout = 20000) {
    requireValue(typeof this.env.ARTIFACT_READ_TOKEN === 'string' && this.env.ARTIFACT_READ_TOKEN.length > 0,
      'Artifact read token is required');
    return this.fetch('https://api.github.com/repos/' + REPO + suffix, {redirect: 'manual',
      signal: AbortSignal.timeout(Math.max(1, Math.ceil(timeout))), headers: {
        Authorization: 'Bearer ' + this.env.ARTIFACT_READ_TOKEN,
        Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28',
        'User-Agent': 'browser-concept-runtime-inputs',
      }});
  }

  async api(suffix, deadline) {
    const remaining = deadline === undefined ? 20000 : deadline - performance.now();
    requireValue(remaining > 0, 'Current job metadata observation budget expired');
    let response;
    try {
      response = await this.request(suffix, Math.min(20000, remaining));
      if (!response.ok) { await response.body?.cancel(); throw new ApiHttpError(response.status); }
      let bytes = 0;
      const blocks = [];
      for await (const block of response.body) {
        bytes += block.length;
        requireValue(bytes <= M, 'Metadata response exceeded budget');
        blocks.push(block);
      }
      if (deadline !== undefined) requireValue(performance.now() <= deadline, 'Current job metadata observation budget expired');
      return JSON.parse(new TextDecoder('utf-8', {fatal: true}).decode(Buffer.concat(blocks)));
    } catch (error) {
      if (error instanceof ApiHttpError || /^(Metadata response|Current job metadata)/.test(error.message)) throw error;
      throw new Error('GitHub metadata request failed');
    }
  }

  async artifactMetadata(aid) {
    try { return [await this.api('/actions/artifacts/' + aid), null]; }
    catch (error) {
      if (!(error instanceof ApiHttpError)) throw error;
      requireValue([404, 410].includes(error.status), 'Artifact metadata failed outside the permitted missing/expired case');
      return [null, 'original_artifact_metadata_http_' + error.status];
    }
  }

  async downloadArchive(aid, destination, expectedBytes, sha) {
    let redirect;
    try { redirect = await this.request('/actions/artifacts/' + aid + '/zip'); }
    catch { throw new Error('Artifact redirect network failure'); }
    await redirect.body?.cancel();
    requireValue(redirect.status === 302, 'Artifact redirect was not HTTP 302');
    const location = storageLocation(redirect.headers.get('location') ?? '');
    const controller = new AbortController();
    const hardStop = setTimeout(() => controller.abort(), 180000);
    let idleStop = setTimeout(() => controller.abort(), 30000);
    let bytes = 0;
    const digest = createHash('sha256');
    try {
      // No Authorization header crosses from GitHub to signed storage, and a
      // second redirect is rejected instead of following to an unverified host.
      const response = await this.fetch(location, {redirect: 'manual', signal: controller.signal});
      if (!response.ok) { await response.body?.cancel(); throw new Error('Artifact storage HTTP response failed'); }
      requireValue(response.body, 'Artifact storage returned no body');
      const checked = new Transform({transform(block, _encoding, callback) {
        clearTimeout(idleStop); idleStop = setTimeout(() => controller.abort(), 30000);
        bytes += block.length;
        if (bytes > expectedBytes) return callback(new Error('Archive exceeded pinned size'));
        digest.update(block); callback(null, block);
      }});
      await pipeline(Readable.fromWeb(response.body), checked, createWriteStream(destination, {flags: 'wx'}),
        {signal: controller.signal});
    } catch (error) {
      if (/^(Archive exceeded|Artifact storage returned)/.test(error.message) || error.code === 'EEXIST') throw error;
      throw new Error('Artifact storage network failure');
    } finally { clearTimeout(hardStop); clearTimeout(idleStop); }
    requireValue(bytes === expectedBytes && digest.digest('hex') === sha, 'Artifact ZIP SHA/size mismatch');
    return {bytes, sha256: sha};
  }

  async retainedMirror() {
    const filename = path.join(this.root, 'reproduction-inputs.zip');
    if (this.mirrorProof !== null) return [filename, this.mirrorProof];
    const run = await this.api('/actions/runs/' + MIRROR.run);
    requireValue(run.id === MIRROR.run && run.head_sha === MIRROR.head && run.head_branch === 'diag/pr36-vcpu-20261120' &&
      run.path === '.github/workflows/pr36-vcpu-gate.yml' && run.status === 'completed' && run.conclusion === 'failure' &&
      run.run_attempt === 1 && run.event === 'push', 'Retained-input source run identity mismatch');
    const artifact = await this.api('/actions/artifacts/' + MIRROR.artifact);
    verifyArtifact(artifact, MIRROR.artifact, MIRROR.name, MIRROR.run, MIRROR.head, MIRROR.bytes, MIRROR.sha256);
    requireValue(artifact.expired === false, 'Retained-input mirror expired');
    const job = await this.api('/actions/jobs/' + MIRROR.job);
    requireValue(job.id === MIRROR.job && job.run_id === MIRROR.run && job.head_sha === MIRROR.head &&
      job.name === 'vCPU browser acceptance (chromium, 1 CPU)' && job.status === 'completed' && job.conclusion === 'failure',
    'Retained-input job identity mismatch');
    const upload = job.steps.filter(step => step.name === 'Retain original verified archives before the source artifacts expire');
    requireValue(upload.length === 1 && upload[0].status === 'completed' && upload[0].conclusion === 'success',
      'Retained-input upload step did not succeed');
    const verified = await this.downloadArchive(MIRROR.artifact, filename, MIRROR.bytes, MIRROR.sha256);
    const archive = await openArchive(filename);
    try { requireValue(archive.entries.length <= 64, 'Retained-input mirror member count exceeded budget'); }
    finally { archive.close(); }
    this.mirrorProof = {...MIRROR, archive: verified, uploadJob: job, uploadStep: upload[0]};
    await writeJson(path.join(this.root, 'retained-input-mirror-provenance.json'), this.mirrorProof);
    return [filename, this.mirrorProof];
  }

  async obtainArchive(label, runId, head, aid, name, bytes, sha) {
    let [metadata, recoveryReason] = await this.artifactMetadata(aid);
    const destination = path.join(this.root, label + '.zip');
    if (metadata !== null) {
      verifyArtifact(metadata, aid, name, runId, head, bytes, sha);
      if (metadata.expired === false) {
        const verified = await this.downloadArchive(aid, destination, bytes, sha);
        return [destination, verified.bytes, null];
      }
      recoveryReason = 'original_artifact_expired';
    }
    const [filename, retained] = await this.retainedMirror();
    const archive = await openArchive(filename);
    let member, actual, proofMember, proofRecord;
    try {
      member = uniqueRetainedMember(archive, label + '.zip');
      actual = await extract(archive, member, destination, bytes);
      requireValue(equal(actual, {bytes, sha256: sha}), 'Retained inner ZIP differs from original pin');
      proofMember = uniqueRetainedMember(archive, label + '-archive-provenance.json');
      const proofDestination = path.join(this.root, 'retained-producer-provenance', label + '.json');
      proofRecord = await extract(archive, proofMember, proofDestination, 4 * M);
      const original = await readJson(proofDestination);
      requireValue(original.artifact === aid && original.run === runId && equal(original.archive, {bytes, sha256: sha}),
        'Retained original producer provenance differs');
      requireValue((Object.hasOwn(original, 'head') ? original.head : original.workflowCommit) === head,
        'Retained original producer commit differs');
      if (label === 'candidate-emulator') {
        requireValue(original.attempt === 1 && original.buildJob?.id === EMULATOR.job, 'Retained emulator producer identity differs');
      } else {
        requireValue(original.label === label && original.artifact_name === name, 'Retained original artifact role differs');
      }
    } finally { archive.close(); }
    const recovery = {kind: 'verified_original_zip_from_retained_mirror', reason: recoveryReason,
      originalArtifact: aid, originalArtifactMetadata: metadata, mirrorArtifact: MIRROR.artifact, mirrorRun: MIRROR.run,
      mirrorArchive: retained.archive, member, originalProducerProvenanceMember: proofMember,
      originalProducerProvenance: proofRecord};
    return [destination, actual.bytes, recovery];
  }

  async observeCurrentJob(mode) {
    const jobs = {
      'Browser concept (chromium)': ['feat/browser-concept-20261120', '.github/workflows/browser-concept.yml', 'chromium'],
      'Browser concept (firefox)': ['feat/browser-concept-20261120', '.github/workflows/browser-concept.yml', 'firefox'],
    };
    const engine = this.env.BROWSER_CONCEPT_ENGINE;
    const jobName = this.env.BROWSER_DIAGNOSTIC_JOB_NAME;
    requireValue(Object.hasOwn(jobs, jobName), 'Unexpected concept browser job name');
    const [branch, workflow, expectedEngine] = jobs[jobName];
    requireValue(engine === expectedEngine, 'Browser engine differs from the exact consumer job');
    requireValue(mode !== '--runtime' || engine === 'chromium', 'Only the first Chromium job materializes retained inputs');
    requireValue(mode !== '--bundle' || engine === 'firefox', 'Only Firefox consumes the completed Chromium site');
    this.currentRun = Number(this.env.GITHUB_RUN_ID);
    const attempt = Number(this.env.GITHUB_RUN_ATTEMPT);
    requireValue(positiveInteger(this.currentRun) && attempt === 1, 'This finite diagnostic forbids workflow reruns');
    const started = performance.now();
    const deadline = started + 30000;
    const observe = async value => {
      const record = {...value, elapsed_ms: Math.round(performance.now() - started), observed_at: new Date().toISOString()};
      await fs.appendFile(path.join(this.root, 'job-start-observations.jsonl'), JSON.stringify(record) + '\n');
      this.log(record);
    };
    const current = await this.api('/actions/runs/' + this.currentRun, deadline);
    await observe({kind: 'current_run', ...pick(current, ['id', 'head_sha', 'head_branch', 'run_attempt', 'event', 'status'])});
    requireValue(current.id === this.currentRun && current.head_sha === this.env.GITHUB_SHA && current.head_branch === branch &&
      current.path === workflow && current.run_attempt === attempt && current.event === 'push', 'Current run immutable identity mismatch');
    requireValue(this.env.BROWSER_DIAGNOSTIC_VCPUS === '2', 'This finite browser diagnostic uses exactly two guest CPUs');
    let expectedJobId, job;
    for (let observation = 1; observation <= 6; observation++) {
      const list = await this.api(`/actions/runs/${this.currentRun}/attempts/${attempt}/jobs?per_page=100`, deadline);
      requireValue(list.total_count <= 100, 'Current run job list exceeded its bounded page');
      const matching = list.jobs.filter(value => value.name === jobName);
      requireValue(matching.length === 1, 'Expected exactly one current diagnostic job');
      job = matching[0];
      await observe({kind: 'current_job', observation,
        ...pick(job, ['id', 'name', 'run_id', 'head_sha', 'status', 'started_at']), reported_job_run_attempt: job.run_attempt ?? null});
      requireValue(positiveInteger(job.id) && job.run_id === this.currentRun &&
        (!Object.hasOwn(job, 'run_attempt') || job.run_attempt === attempt) && job.head_sha === current.head_sha,
      'Current job immutable identity mismatch');
      expectedJobId ??= job.id;
      requireValue(job.id === expectedJobId, 'Current job identity changed between observations');
      requireValue(['queued', 'in_progress'].includes(job.status), 'Current job is no longer eligible to start');
      requireValue(job.started_at === null || (typeof job.started_at === 'string' && job.started_at.endsWith('Z') &&
        Number.isFinite(Date.parse(job.started_at))), 'Current job start metadata is malformed');
      if (job.status === 'in_progress' && job.started_at !== null) break;
      requireValue(observation < 6 && performance.now() + 2000 < deadline,
        'Current job start metadata did not become available within its observation budget');
      await delay(2000);
    }
    requireValue(performance.now() <= deadline, 'Current job metadata observation budget expired');
    this.jobTiming = {...pick(job, ['id', 'name', 'run_id', 'head_sha', 'started_at']), run_attempt: current.run_attempt,
      source: 'GitHub Actions current job API', observed_at: new Date().toISOString()};
    await writeJson(path.join(this.root, 'job-timing.json'), this.jobTiming);
    this.log({current_job_timing: this.jobTiming});
  }

  async currentArtifact(kind, idVariable, digestVariable, expectedName, expectedJob, limit) {
    const aidText = this.env[idVariable];
    requireValue(typeof aidText === 'string' && /^[1-9][0-9]*$/.test(aidText) && positiveInteger(Number(aidText)),
      'Malformed current artifact ID');
    const aid = Number(aidText);
    const sha = (this.env[digestVariable] ?? '').replace(/^sha256:/, '');
    requireValue(/^[0-9a-f]{64}$/.test(sha), 'Malformed current artifact digest');
    const metadata = await this.api('/actions/artifacts/' + aid);
    const bytes = metadata.size_in_bytes;
    requireValue(positiveInteger(bytes) && bytes <= limit, 'Current artifact exceeded its size budget');
    verifyArtifact(metadata, aid, expectedName, this.currentRun, this.env.GITHUB_SHA, bytes, sha);
    requireValue(metadata.expired === false, 'Current artifact has expired');
    const list = await this.api(`/actions/runs/${this.currentRun}/attempts/1/jobs?per_page=100`);
    requireValue(list.total_count <= 100, 'Current producer job list exceeded budget');
    const matches = list.jobs.filter(job => job.name === expectedJob);
    requireValue(matches.length === 1, 'Expected exactly one current artifact producer job');
    const job = matches[0];
    requireValue(job.run_id === this.currentRun && job.head_sha === this.env.GITHUB_SHA && job.status === 'completed' &&
      job.conclusion === 'success' && (!Object.hasOwn(job, 'run_attempt') || job.run_attempt === 1),
    'Current artifact producer did not complete successfully at this exact source');
    const filename = path.join(this.root, kind + '.zip');
    const actual = await this.downloadArchive(aid, filename, bytes, sha);
    const proof = {kind, artifact: aid, artifactName: expectedName, archive: actual,
      producer: {runId: this.currentRun, runAttempt: 1, workflowCommit: this.env.GITHUB_SHA, job},
      consumerJob: this.jobTiming, members: {}};
    return [filename, proof];
  }

  async restoreConceptBinary() {
    const [filename, proof] = await this.currentArtifact('concept-native', 'CONCEPT_NATIVE_ARTIFACT_ID',
      'CONCEPT_NATIVE_ARTIFACT_DIGEST', 'browser-concept-native', 'Build browser concept binary', 64 * M);
    const destination = path.resolve('artifacts/browser-concept-native');
    await fs.mkdir(path.dirname(destination), {recursive: true});
    await fs.mkdir(destination);
    const expected = ['concept', 'build.json', 'source-manifest.json', 'builder-image.json', 'build.log', 'rustfmt.log',
      'clippy.log', 'unit-tests.log', 'cargo-build.log', 'rustc-version.txt', 'cargo-version.txt', 'elf.txt'];
    const archive = await openArchive(filename);
    try {
      requireValue(sameNames(archive.entries.filter(entry => !isDirectory(entry)).map(entry => entry.fileName), expected),
        'Unexpected concept producer artifact members');
      requireValue(sumSizes(archive.entries) <= 128 * M, 'Concept artifact expansion exceeded budget');
      for (const name of expected.sort()) proof.members[name] = await extract(archive, name, path.join(destination, name), 64 * M, name.endsWith('.log'));
    } finally { archive.close(); }
    const build = await readJson(path.join(destination, 'build.json'));
    requireValue(build.schemaVersion === 1 && build.kind === 'browser_concept_native' &&
      build.sourceCommit === this.env.GITHUB_SHA && build.target === 'x86_64-unknown-linux-gnu' &&
      equal(build.binary, proof.members.concept) && equal(build.sourceManifest, proof.members['source-manifest.json']),
    'Concept source/binary provenance mismatch');
    requireValue(equal(build.producer, {runId: this.currentRun, runAttempt: 1, workflowCommit: this.env.GITHUB_SHA,
      jobName: 'Build browser concept binary'}), 'Concept producer identity mismatch');
    const source = await readJson(path.join(destination, 'source-manifest.json'));
    requireValue(source.sourceCommit === build.sourceCommit && source.sourceTree === build.sourceTree,
      'Concept source manifest identity mismatch');
    for (const [name, expectedRecord] of Object.entries(source.files)) {
      requireValue(!path.posix.isAbsolute(name) && !name.split('/').includes('..') && !name.includes('\\') && !name.includes('\0'),
        'Unsafe concept source path');
      requireValue(equal(await fileRecord(name), expectedRecord), 'Concept source changed between producer and browser: ' + name);
    }
    for (const [name, log] of [['rustfmt', 'rustfmt.log'], ['clippy', 'clippy.log'], ['unitTests', 'unit-tests.log']]) {
      requireValue(build.checks[name].passed === true && equal(build.checks[name].log, proof.members[log]),
        'Concept native check proof mismatch: ' + name);
    }
    requireValue(equal(build.model, source.files['browser-concept/model.json']), 'Concept model identity mismatch');
    await fs.chmod(path.join(destination, 'concept'), 0o555);
    await writeJson(path.join(this.root, 'concept-native-archive-provenance.json'), proof);
    await fs.unlink(filename);
    this.log({verifiedCurrentArtifact: 'concept-native', artifact: proof.artifact, binary: build.binary, browserAcceptanceVerified: false});
  }

  async restoreChromiumBundle() {
    const [filename, proof] = await this.currentArtifact('concept-chromium-site', 'CONCEPT_SITE_ARTIFACT_ID',
      'CONCEPT_SITE_ARTIFACT_DIGEST', 'browser-concept-site-chromium', 'Browser concept (chromium)', 1024 * M);
    const destination = path.resolve('browser-output/concept');
    await fs.mkdir(path.dirname(destination), {recursive: true});
    await fs.mkdir(destination);
    const archive = await openArchive(filename);
    try {
      const files = archive.entries.filter(entry => !isDirectory(entry));
      requireValue(files.length > 0 && files.length <= 2048 && sumSizes(files) <= 2048 * M, 'Concept site expansion exceeded budget');
      for (const entry of files) proof.members[entry.fileName] = await extract(archive, entry.fileName,
        path.join(destination, entry.fileName), 1024 * M, true);
    } finally { archive.close(); }
    const required = ['index.html', 'launcher.js', 'runtime/startup-manifest.json', 'runtime/concept', 'runtime/openfga',
      'runtime/qemu-system-x86_64.data', 'runtime/qemu-system-x86_64.wasm', 'provenance/assembly.json',
      'provenance/concept-build.json', 'provenance/native-proof.json'];
    requireValue(required.every(name => Object.hasOwn(proof.members, name)), 'Chromium site artifact is incomplete');
    const manifest = await readJson(path.join(destination, 'runtime/startup-manifest.json'));
    requireValue(manifest.kind === 'browser_concept_startup' && manifest.schemaVersion === 1 &&
      manifest.conceptSourceCommit === this.env.GITHUB_SHA, 'Chromium site source identity mismatch');
    for (const [name, expectedRecord] of Object.entries(manifest.files)) {
      requireValue(equal(proof.members[name], expectedRecord), 'Chromium payload identity mismatch: ' + name);
    }
    await writeJson(path.join(this.root, 'concept-chromium-site-archive-provenance.json'), proof);
    await fs.unlink(filename);
    this.log({verifiedCurrentArtifact: 'concept-chromium-site', artifact: proof.artifact,
      files: Object.keys(proof.members).length, firefoxAcceptanceVerified: false});
  }

  async restoreOriginal(spec) {
    const [label, runId, head, aid, name, bytes, sha, conclusion, event] = spec;
    const run = await this.api('/actions/runs/' + runId);
    requireValue(run.id === runId && run.status === 'completed' && run.conclusion === conclusion &&
      run.head_sha === head && run.run_attempt === 1 && run.event === event, 'Pinned source run identity mismatch');
    let nativeJob = null;
    if (label === 'native') {
      nativeJob = await this.api('/actions/jobs/' + native.job);
      requireValue(nativeJob.id === native.job && nativeJob.run_id === runId && nativeJob.head_sha === head &&
        nativeJob.name === 'native' && nativeJob.status === 'completed' && nativeJob.conclusion === 'success',
      'The fresh native job did not succeed');
      for (const expectedStep of [
        'Check source identity, repository contract and both producer scripts',
        'Build and regression-test the fresh OpenFGA .10 producer',
        'Bind the fresh binary to exact inputs and actual regular and race tests',
        'Execute the versioned native bootstrap producer with this exact OpenFGA output',
        'Verify all four outputs and their real native OpenFGA provenance',
      ]) {
        const completed = nativeJob.steps.filter(step => step.name === expectedStep);
        requireValue(completed.length === 1 && completed[0].status === 'completed' && completed[0].conclusion === 'success',
          'Missing successful native proof step');
      }
    }
    const [filename, count, recovery] = await this.obtainArchive(label, runId, head, aid, name, bytes, sha);
    const members = {};
    const archive = await openArchive(filename);
    try {
      let selected;
      if (label === 'lightweight') {
        const prefix = 'artifacts/browser-log-transport-candidate/';
        const scenarios = ['synchronous_uart_burst', 'separate_tasks_ack_gate', 'separate_tasks_unblocked',
          'finish_partial_utf16', 'render_error_at_cutoff', 'delivery_error', 'pending_chunk_bound'];
        const sourcePaths = ['browser-single/pages/log-stream.mjs', 'browser-single/pages/launcher.js',
          'browser-single/log-stream.test.mjs'];
        const proofPaths = [prefix + 'summary.json', prefix + 'node-tests.log', 'diagnostics/check-log-transport-candidate.mjs',
          ...sourcePaths.map(source => 'source-candidate/' + source)];
        const expected = [...proofPaths];
        for (const browser of ['chromium', 'firefox']) for (const scenario of scenarios) {
          for (const suffix of ['.json', '-console.txt', '-stream.jsonl']) expected.push(prefix + browser + '-' + scenario + suffix);
        }
        requireValue(sameNames(archive.entries.filter(entry => !isDirectory(entry)).map(entry => entry.fileName), expected),
          'Unexpected lightweight proof artifact members');
        selected = proofPaths.map(member => [member, path.join(this.root, 'lightweight-evidence', member), 2 * M]);
      } else if (label === 'retained') {
        selected = [
          ['browser-output/wasm/runtime-source.zip', path.join(this.root, 'runtime-source.zip'), 384 * M],
          ['browser-output/wasm/runtime/derived-runtime.json', path.join(this.root, 'retained/derived-runtime.json'), 4 * M],
          ['browser-output/wasm/runtime/startup-manifest.json', path.join(this.root, 'retained/startup-manifest.json'), 4 * M],
        ];
        const subtree = archive.entries.filter(entry => entry.fileName.startsWith('artifacts/browser-emulator/') && !isDirectory(entry));
        requireValue(subtree.length > 0 && subtree.length <= 256, 'Unexpected retained proof subtree size');
        selected.push(...subtree.map(entry => [entry.fileName, path.join(this.sourceRoot, entry.fileName), 128 * M]));
      } else if (label === 'kernel') {
        const destination = path.join(this.sourceRoot, 'artifacts/browser-kernel');
        selected = [
          ['kernel/bzImage', 'bzImage', 32 * M], ['kernel/linux.config', 'linux.config', 2 * M],
          ['kernel/upstream.config', 'upstream.config', 2 * M], ['kernel/ipc-baseline.config', 'ipc-baseline.config', 2 * M],
          ['kernel/acpi.config.diff', 'acpi.config.diff', 65536], ['kernel/acpi-baseline.config', 'acpi-baseline.config', 2 * M],
          ['kernel/rng.config.diff', 'rng.config.diff', 65536], ['kernel/COPYING', 'COPYING', M], ['kernel/GPL-2.0', 'GPL-2.0', M],
          ['kernel/SOURCE.txt', 'SOURCE.txt', 65536], ['kernel/build.sha256', 'build.sha256', 65536],
          ['metadata.json', 'metadata.json', M], ['Kernel.Dockerfile', 'Kernel.Dockerfile', 65536],
          ['source-commit.txt', 'source-commit.txt', 128], ['workflow-commit.txt', 'workflow-commit.txt', 128],
        ].map(([member, relative, limit]) => [member, path.join(destination, relative), limit]);
      } else if (label === 'native') {
        // upload-artifact's common root is producer/artifacts, not producer.
        const subtree = archive.entries.filter(entry => entry.fileName.startsWith('browser-openfga/') && !isDirectory(entry));
        requireValue(subtree.length > 0 && subtree.length <= 256, 'Unexpected new OpenFGA proof subtree size');
        selected = subtree.map(entry => [entry.fileName, path.join(this.sourceRoot, 'artifacts', entry.fileName), 128 * M]);
        selected.push(...[
          ['openfga10-verifier-input.json', 4 * M], ['openfga10-build-verification.json', 4 * M],
          ['browser-openfga-build.log', 64 * M], ['openfga10-native-startup-timing.json', M],
          ['browser-bootstrap-proof/native-result.json', 4 * M], ['browser-bootstrap-proof/openfga-build.json', 4 * M],
        ].map(([member, limit]) => [member, path.join(this.root, 'native-evidence', member), limit]));
      } else if (label === 'bootstrap') {
        const expected = {bootstrap: 128 * M, 'build.json': 4 * M, 'native-tests.log': 16 * M, 'SOURCE.txt': 65536};
        requireValue(sameNames(archive.entries.filter(entry => !isDirectory(entry)).map(entry => entry.fileName), Object.keys(expected)),
          'Unexpected native bootstrap artifact members');
        selected = Object.entries(expected).map(([member, limit]) => [member, path.join(this.sourceRoot, 'artifacts/browser-bootstrap', member), limit]);
      } else throw new Error('Unexpected archive role');
      let total = 0;
      for (const [member, destination, limit] of selected) {
        total += getEntry(archive, member).uncompressedSize;
        requireValue(total <= 768 * M, 'Selected artifact members exceeded budget');
        members[member] = await extract(archive, member, destination, limit);
      }
    } finally { archive.close(); }

    if (label === 'retained') {
      const manifest = await readJson(path.join(this.sourceRoot, 'browser-single/pages/runtime-manifest.json'));
      const nested = path.join(this.root, 'runtime-source.zip');
      requireValue(members['browser-output/wasm/runtime-source.zip'].sha256 === manifest.archive_sha256,
        'Original converter archive changed');
      const runtime = await openArchive(nested);
      try {
        requireValue(sameNames(runtime.byName.keys(), Object.keys(manifest.files)), 'Unexpected converter ZIP members');
        for (const [member, expected] of Object.entries(manifest.files)) {
          requireValue(['out.js', 'load.js', 'arg-module.js', 'qemu-system-x86_64.wasm', 'qemu-system-x86_64.data'].includes(member),
            'Unexpected converter runtime member');
          const actual = await extract(runtime, member, path.join(this.root, 'original-runtime', member), expected.bytes);
          requireValue(equal(actual, expected), 'Original converter runtime SHA/size mismatch: ' + member);
        }
      } finally { runtime.close(); }
    } else if (label === 'kernel') {
      const candidate = '716b77356dd4aab2747d4e37c5da723f3b7a764f';
      const destination = path.join(this.sourceRoot, 'artifacts/browser-kernel');
      requireValue((await fs.readFile(path.join(destination, 'source-commit.txt'), 'utf8')).trim() === candidate &&
        (await fs.readFile(path.join(destination, 'workflow-commit.txt'), 'utf8')).trim() === head, 'Kernel build source provenance changed');
      const built = await readJson(path.join(destination, 'metadata.json'));
      requireValue(built.source_commit === candidate && built.workflow_commit === head && built.run_id === String(runId) &&
        built.run_attempt === '1' && built.kernel_candidate_built === true && built.browser_acceptance_verified === false &&
        built.entropy_delivery_verified === false && built.baseline_matches_previous_kernel_config === true &&
        built.exactly_one_config_change === true && built.exact_rng_config_diff === true && built.previous_acpi_diff_preserved === true &&
        built.kernel_hash_manifest_matches === true && built.source_checks_outcome === 'success' && built.build_outcome === 'success' &&
        equal(built.missing_config, []) && equal(built.missing_files, []) &&
        built.recipe_git_blob === 'aeb75dc1f17528939559ffe8f8dd15746c0c2a0e', 'Kernel build metadata identity mismatch');
    } else if (label === 'native') {
      const inputs = await readJson(path.join(this.root, 'native-evidence/openfga10-verifier-input.json'));
      requireValue(inputs.schemaVersion === 3 && inputs.experiment === 'fresh_openfga10_two_walk_schema_reachability_and_bootstrap_deny' &&
        inputs.producerSource === 'bb8fed250bdee9931f7702ae2475d28e5e9eed8f' &&
        inputs.runtimeSource === '444b069daf16e9606a0bbf1ea0aea7f4d61b97d8' && inputs.workflowCommit === head &&
        inputs.runId === String(runId) && inputs.runAttempt === '1' && inputs.repositoryRuntimeContract === 'passed' &&
        inputs.scriptSyntaxAndShellCheck === 'passed' && inputs.browserAcceptanceVerified === false,
      'Fresh native source/input provenance mismatch');
      requireValue(equal(members['browser-openfga/openfga'], native.binaries.openfga), 'The fresh OpenFGA binary bytes differ');
    } else if (label === 'bootstrap') {
      const built = await readJson(path.join(this.sourceRoot, 'artifacts/browser-bootstrap/build.json'));
      requireValue(built.source_commit === '207d15fab685d3adb9c5d51a048982c2236ae75c' &&
        built.packaging_commit === '4b7a1e1933083232fb8305a60a47bdd3c32e6689' && built.workflow_commit === head &&
        built.run_id === String(runId) && built.run_attempt === '1' && built.browser_acceptance_verified === false &&
        built.native_tests === 'passed' && built.empty_store_check === 'one_real_higher_consistency_ownership_deny' &&
        built.empty_store_check_default === false && built.browser_empty_store_check === true &&
        built.empty_store_check_timeout_seconds === 5 && built.empty_store_check_tests === 'passed' &&
        equal({bytes: built.binary_bytes, sha256: built.binary_sha256}, native.binaries.bootstrap) &&
        built.binary_bytes === members.bootstrap.bytes && built.binary_sha256 === members.bootstrap.sha256 &&
        built.native_test_log_sha256 === members['native-tests.log'].sha256, 'Native bootstrap proof/binary identity mismatch');
    }
    const provenance = {label, run: runId, head, artifact: aid, artifact_name: name,
      archive: {bytes: count, sha256: sha}, members};
    if (recovery !== null) provenance.recovery = recovery;
    if (nativeJob !== null) {
      provenance.native_run_attempt = run.run_attempt;
      provenance.native_job = pick(nativeJob, ['id', 'run_id', 'head_sha', 'name', 'status', 'conclusion', 'started_at', 'completed_at', 'steps']);
    }
    await writeJson(path.join(this.root, label + '-archive-provenance.json'), provenance);
    await fs.unlink(filename);
    this.log({verified_archive: label, artifact: aid, bytes: count, sha256: sha});
  }

  async restoreEmulator() {
    const {artifact: aid, bytes, sha256: sha, run: runId, head, name, job: jobId} = EMULATOR;
    // The whole original run failed its browser gate; only this exact build job passed.
    const producer = await this.api('/actions/runs/' + runId);
    requireValue(producer.id === runId && producer.run_attempt === 1 && producer.head_sha === head &&
      producer.head_branch === 'diag/pr36-idle-worker-gc-20261117' && producer.path === '.github/workflows/pr36-idle-gc-gate.yml' &&
      producer.status === 'completed' && producer.conclusion === 'failure' && producer.event === 'push',
    'Original emulator producer run identity differs');
    const list = await this.api(`/actions/runs/${runId}/attempts/1/jobs?per_page=100`);
    requireValue(list.total_count <= 100, 'Emulator producer job list exceeded its bounded page');
    const matches = list.jobs.filter(job => job.name === 'Build QEMU idle-worker GC candidate');
    requireValue(matches.length === 1, 'Expected exactly one emulator build job');
    const job = matches[0];
    requireValue(job.id === jobId && job.run_id === runId && job.status === 'completed' && job.conclusion === 'success' &&
      job.head_sha === head, 'Original emulator build job did not succeed');
    const [filename, count, recovery] = await this.obtainArchive('candidate-emulator', runId, head, aid, name, bytes, sha);
    const destination = 'candidate-emulator';
    await fs.mkdir(destination);
    const members = {};
    const archive = await openArchive(filename);
    try {
      requireValue(archive.entries.length <= 512, 'Emulator artifact member count exceeded budget');
      let total = 0;
      for (const entry of archive.entries) {
        if (isDirectory(entry)) continue;
        requireValue(entry.uncompressedSize >= 0 && entry.uncompressedSize <= 128 * M, 'Invalid emulator artifact member size');
        total += entry.uncompressedSize;
        requireValue(total <= 256 * M, 'Emulator artifact expansion exceeded budget');
        members[entry.fileName] = await extract(archive, entry.fileName, path.join(destination, entry.fileName), 128 * M, true);
      }
    } finally { archive.close(); }
    const record = {artifact: aid, archive: {bytes: count, sha256: sha}, run: runId, attempt: 1, workflowCommit: head, buildJob: job,
      members, reuseConsumer: {run: this.currentRun, attempt: 1, workflowCommit: this.env.GITHUB_SHA}};
    if (recovery !== null) record.recovery = recovery;
    await writeJson(path.join(this.root, 'candidate-emulator-archive-provenance.json'), record);
    await fs.unlink(filename);
    this.log({candidateArtifact: aid, sha256: sha, members: Object.keys(members).length, buildJob: job.id,
      recoveredOriginalBytes: recovery !== null, browserAcceptanceVerified: false, vmStarts: 0});
  }

  async run(args = []) {
    const mode = args[0] ?? '--runtime';
    requireValue(args.length <= 1 && ['--runtime', '--bundle', '--job-only'].includes(mode), 'Expected --runtime, --bundle or --job-only');
    verifyPins();
    await fs.mkdir(this.root);
    await this.observeCurrentJob(mode);
    if (mode === '--job-only') return;
    if (mode === '--bundle') { await this.restoreChromiumBundle(); return; }
    for (const spec of specs) await this.restoreOriginal(spec);
    await this.restoreEmulator();
    await this.restoreConceptBinary();
    // Archives preserve verified bytes, not executable mode. Native integration
    // executes this exact retained ELF before VM materialization.
    await fs.chmod(path.join(this.sourceRoot, 'artifacts/browser-openfga/openfga'), 0o555);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { await new RuntimeInputReader().run(process.argv.slice(2)); }
  catch (error) { console.error('Runtime input restoration failed: ' + error.message); process.exitCode = 1; }
}
