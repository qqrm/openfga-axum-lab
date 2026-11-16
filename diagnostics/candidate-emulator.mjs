// Shared source/build identity checks for the finite QEMU-only candidate run.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream, promises as fs} from 'node:fs';
import {execFileSync} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
export const baseline = '444b069daf16e9606a0bbf1ea0aea7f4d61b97d8';
export const oldBackend = 'e0b26bdc8d6d5b987061f9090ae0b62b76c34e16';
export const git = (cwd, ...args) => execFileSync('git', args, {cwd,encoding:'utf8'}).trim();
export async function record(filename) {
  const hash=createHash('sha256'); let bytes=0;
  for await (const chunk of createReadStream(filename)) {hash.update(chunk);bytes+=chunk.length;}
  return {bytes,sha256:hash.digest('hex')};
}
export async function verifySource(source) {
  const expected=process.env.EMULATOR_SOURCE_SHA;
  assert.match(expected ?? '', /^[0-9a-f]{40}$/, 'Pin the reviewed immutable emulator source before running');
  assert.equal(git(source,'rev-parse','HEAD'),expected);
  git(source,'diff','--exit-code','HEAD','--');
  git(source,'merge-base','--is-ancestor',baseline,expected);
  const changed=git(source,'diff','--name-only',baseline,expected).split('\n').filter(Boolean);
  assert(changed.length>0,'The candidate must contain the reviewed fix');
  assert(changed.every(name=>name.startsWith('browser-single/qemu/') || name==='browser-single/prepare-emulator.mjs'),
    'Emulator-only candidate changed other runtime/application inputs');
  const names=git(source,'ls-tree','-r','--name-only','HEAD','--','browser-single/qemu','browser-single/prepare-emulator.mjs').split('\n');
  const files={};
  for (const name of names) {
    const entry=git(source,'ls-tree','HEAD','--',name);
    const match=/^(100644|100755) blob ([0-9a-f]{40})\t/.exec(entry);
    assert(match,'Expected a regular emulator source file: '+name);
    assert.equal(git(source,'hash-object','--',name),match[2]);
    files[name]={mode:match[1],gitBlob:match[2],...await record(path.join(source,name))};
  }
  return {baselineSource:baseline,candidateSource:expected,changedFiles:changed,sourceFiles:files};
}
const inputs={patch_sha256:'instance-budget.patch',builder_sha256:'build.sh',
  dockerfile_appendix_sha256:'Dockerfile.appendix',budget_test_sha256:'check-instance-budget.mjs',
  diagnostics_test_sha256:'check-runtime-diagnostics.mjs',budget_bridge_test_sha256:'check-instance-budget-bridge.mjs',
  multiply_patch_sha256:'i32-multiply.patch',multiply_test_sha256:'check-i32-multiply.mjs',
  barrier_patch_sha256:'memory-barrier.patch',barrier_test_sha256:'check-memory-barrier.mjs',
  idle_gc_patch_sha256:'idle-gc-yield.patch',idle_gc_test_sha256:'check-idle-gc.mjs',
  idle_gc_bridge_test_sha256:'check-idle-gc.mjs'};
export async function verifyBuild(source,directory) {
  const build=JSON.parse(await fs.readFile(path.join(directory,'build.json'),'utf8'));
  assert.match(build.backend_postimage_blob ?? '',/^[0-9a-f]{40}$/);
  assert.notEqual(build.backend_postimage_blob,oldBackend,'The old backend cannot satisfy the fix candidate');
  assert.equal(build.idle_gc_preimages?.['tcg/wasm32.c'],oldBackend);
  assert.equal(build.idle_gc_postimages?.['tcg/wasm32.c'],build.backend_postimage_blob);
  for (const [key,value] of Object.entries({
    converter_commit:'087c313479d51cdf255e4f28ded6ae7d8141297b',
    converter_dockerfile_sha256:'f122e849fcb79d718a73dbef6c6cff6b2b71fd3032c8ea7686dc26c007caa508',
    qemu_commit:'8604ed49a3cde392890b014a8d5a959c8a2fe72a',emscripten_version:'4.0.10',
    emsdk_image:'emscripten/emsdk:4.0.10@sha256:90b757eb11fa9a0e3ce4d2d9f76d932a56018e4accc37b5a28b2783751e60eb7',
    max_instance_alive_after:15000,firefox_instance_limit:8192,chromium_instance_limit:15000,
    max_instance_running_local:2048,instance_reservation:'atomic_compare_exchange',
    idle_gc_policy:'pending_local_finalizers_timed_idle_wait',idle_gc_poll_ms:20,
    idle_gc_tests:'passed',idle_gc_bridge_tests:'passed',
    instance_budget_tests:'passed',instance_budget_bridge_tests:'passed',runtime_diagnostics_tests:'passed',
    i32_multiply_tests:'passed',memory_barrier_tests:'passed',browser_acceptance:'not_run'
  })) assert.equal(build[key],value,'Unexpected candidate build '+key);
  for (const [key,name] of Object.entries(inputs)) {
    const actual=await record(path.join(source,'browser-single/qemu',name));
    assert.equal(build[key],actual.sha256,'Build was produced from different source: '+name);
    assert.deepEqual(await record(path.join(directory,name)),actual,'Exported build input differs: '+name);
  }
  for (const [key,directoryName,kind] of [
    ['idle_gc_proof_sha256','idle-gc-proof','compiled_native_idle_gc_locking'],
    ['idle_gc_bridge_proof_sha256','idle-gc-bridge-proof','compiled_emscripten_idle_gc_queue']
  ]) {
    const summaryPath=path.join(directory,directoryName,'summary.json');
    assert.equal((await record(summaryPath)).sha256,build[key],'Changed idle-GC proof: '+directoryName);
    const summary=JSON.parse(await fs.readFile(summaryPath,'utf8'));
    assert.equal(summary.kind,kind);
  }
  assert.deepEqual(Object.keys(build.files).sort(),['out.js','qemu-system-x86_64.wasm']);
  for (const [name,expected] of Object.entries(build.files)) assert.deepEqual(await record(path.join(directory,name)),expected);
  return build;
}
if (process.argv[1] && path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const [source,directory]=process.argv.slice(2);
  assert(source,'usage: candidate-emulator.mjs SOURCE [BUILD_DIR]');
  const identity=await verifySource(path.resolve(source));
  if (directory) {
    const build=await verifyBuild(path.resolve(source),path.resolve(directory));
    const provenance={...identity,workflowCommit:process.env.GITHUB_SHA,runId:Number(process.env.GITHUB_RUN_ID),
      runAttempt:Number(process.env.GITHUB_RUN_ATTEMPT),build:await record(path.join(directory,'build.json')),
      files:build.files,createdAt:new Date().toISOString(),browserAcceptanceVerified:false};
    assert.match(provenance.workflowCommit ?? '',/^[0-9a-f]{40}$/);
    assert(Number.isSafeInteger(provenance.runId)&&provenance.runId>0);
    assert.equal(provenance.runAttempt,1);
    await fs.writeFile(path.join(directory,'candidate-provenance.json'),JSON.stringify(provenance,null,2)+'\n');
  }
  console.log(JSON.stringify({candidateSource:identity.candidateSource,changedFiles:identity.changedFiles,
    buildVerified:Boolean(directory),browserAcceptanceVerified:false}));
}
