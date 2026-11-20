// Host-side provenance for the concept-only native build. No runtime is executed.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';

const sourceDirectories = ['src/', 'tools/', 'assets/', 'db/', 'migrations/', 'fga/', 'benches/'];
const sourceFiles = new Set(['.dockerignore', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
  'browser-concept/Dockerfile', 'browser-concept/build-native.sh', 'browser-concept/build-provenance.mjs']);
const git = (...args) => execFileSync('git', args, {encoding: 'utf8'}).trim();
const readJSON = async filename => JSON.parse(await fs.readFile(filename, 'utf8'));
const writeJSON = (filename, value) => fs.writeFile(filename, JSON.stringify(value, null, 2) + '\n');

export async function record(filename) {
  assert((await fs.lstat(filename)).isFile(), 'Expected a regular file: ' + filename);
  const bytes = await fs.readFile(filename);
  return {bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex')};
}

export async function prepare(output) {
  const tracked = new TextDecoder('utf-8', {fatal: true})
    .decode(execFileSync('git', ['ls-files', '-z'])).split('\0').filter(Boolean);
  const selected = tracked.filter(name => sourceFiles.has(name) ||
    sourceDirectories.some(directory => name.startsWith(directory)) ||
    (name.startsWith('browser-concept/') && !name.slice('browser-concept/'.length).includes('/') && name.endsWith('.json')));
  assert(selected.includes('src/bin/browser-concept.rs') && selected.includes('browser-concept/model.json'));
  const files = {};
  for (const name of selected.sort()) files[name] = await record(name);
  const proof = {sourceCommit: git('rev-parse', 'HEAD'), sourceTree: git('rev-parse', 'HEAD^{tree}'), files};
  await writeJSON(path.join(output, 'source-manifest.json'), proof);
  return proof;
}

// Match the numeric tuple ordering used for glibc symbol versions. A longer
// tuple with an equal prefix is greater, so 2.36.1 cannot pass a 2.36 ceiling.
function compareVersions(left, right) {
  for (let index = 0; index < Math.min(left.length, right.length); index++) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return left.length - right.length;
}

export function verifyElf(binary, description) {
  assert(binary.length >= 20 && binary.subarray(0, 6).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1])) &&
    binary.readUInt16LE(18) === 62, 'Expected Linux x86-64 ELF');
  const versions = [...description.matchAll(/GLIBC_(\d+(?:\.\d+)+)/g)].map(match => match[1].split('.').map(Number));
  assert(versions.length > 0 && versions.every(version => version.every(Number.isSafeInteger)),
    'ELF description did not contain valid glibc symbol versions');
  const maximum = versions.reduce((left, right) => compareVersions(left, right) >= 0 ? left : right);
  assert(compareVersions(maximum, [2, 36]) <= 0, 'Binary requires a newer libc than the retained Bookworm guest');
  return maximum.join('.');
}

function positiveInteger(value, name) {
  assert(/^[1-9][0-9]*$/.test(value ?? ''), 'Missing or invalid ' + name);
  const number = Number(value);
  assert(Number.isSafeInteger(number), name + ' exceeds the safe integer range');
  return number;
}

export async function finish(output, tag, digest, env = process.env) {
  const source = await readJSON(path.join(output, 'source-manifest.json'));
  assert.equal(source.sourceCommit, env.GITHUB_SHA);
  for (const [name, expected] of Object.entries(source.files)) {
    assert.deepEqual(await record(name), expected, 'Source input changed during build: ' + name);
  }
  const images = await readJSON(path.join(output, 'builder-image.json'));
  assert(Array.isArray(images) && images.length === 1);
  const image = images[0];
  assert(image.Os === 'linux' && image.Architecture === 'amd64' && image.RepoDigests.includes(digest));
  const maximumRequiredGlibc = verifyElf(await fs.readFile(path.join(output, 'concept')),
    await fs.readFile(path.join(output, 'elf.txt'), 'utf8'));
  const tests = await fs.readFile(path.join(output, 'unit-tests.log'), 'utf8');
  const match = /test result: ok\. ([1-9][0-9]*) passed; 0 failed;/.exec(tests);
  assert(match, 'Focused concept behavioral tests did not report a nonempty successful suite');
  const compiler = await fs.readFile(path.join(output, 'rustc-version.txt'), 'utf8');
  assert(/^release: 1\.99\.1$/m.test(compiler));
  const checks = {};
  for (const [name, logfile] of [['rustfmt', 'rustfmt.log'], ['clippy', 'clippy.log'], ['unitTests', 'unit-tests.log']]) {
    checks[name] = {passed: true, log: await record(path.join(output, logfile))};
  }
  checks.unitTests.passedTests = positiveInteger(match[1], 'successful unit-test count');
  const runId = positiveInteger(env.GITHUB_RUN_ID, 'GITHUB_RUN_ID');
  const runAttempt = positiveInteger(env.GITHUB_RUN_ATTEMPT, 'GITHUB_RUN_ATTEMPT');
  assert.equal(runAttempt, 1);
  const proof = {
    schemaVersion: 1, kind: 'browser_concept_native', sourceCommit: source.sourceCommit, sourceTree: source.sourceTree,
    target: 'x86_64-unknown-linux-gnu', binary: await record(path.join(output, 'concept')),
    model: await record('browser-concept/model.json'), cargoToml: await record('Cargo.toml'), cargoLock: await record('Cargo.lock'),
    sourceManifest: await record(path.join(output, 'source-manifest.json')), sourceInputCount: Object.keys(source.files).length,
    builder: {tag, digest, imageId: image.Id, inspection: await record(path.join(output, 'builder-image.json'))},
    compiler, cargo: (await fs.readFile(path.join(output, 'cargo-version.txt'), 'utf8')).trim(),
    buildCommand: 'cargo build --locked --release --bin browser-concept', checks, maximumRequiredGlibc,
    producer: {runId, runAttempt, workflowCommit: env.GITHUB_SHA, jobName: 'Build browser concept binary'},
    nativeIntegrationVerified: false, browserAcceptanceVerified: false, rootfsRebuilt: false, emulatorRebuilt: false,
  };
  await writeJSON(path.join(output, 'build.json'), proof);
  execFileSync('git', ['diff', '--exit-code', 'HEAD', '--'], {stdio: 'pipe'});
  return proof;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const [command, output, tag, digest, ...extra] = process.argv.slice(2);
  assert(output && extra.length === 0, 'Usage: build-provenance.mjs prepare OUTPUT | finish OUTPUT TAG DIGEST');
  if (command === 'prepare') {
    assert(tag === undefined && digest === undefined);
    await prepare(output);
  } else {
    assert(command === 'finish' && tag && digest, 'Expected prepare or finish command');
    const proof = await finish(output, tag, digest);
    console.log(JSON.stringify({kind: proof.kind, sourceCommit: proof.sourceCommit, binary: proof.binary,
      passedUnitTests: proof.checks.unitTests.passedTests, nativeIntegrationVerified: false}));
  }
}
