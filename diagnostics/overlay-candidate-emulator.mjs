// Materialize 444b unchanged first; then apply and prove one emulator-only delta.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {baseline,git,record,verifySource,verifyBuild} from './candidate-emulator.mjs';
const source=path.resolve('source-candidate');
const candidate=path.resolve('emulator-source');
const buildDirectory=path.resolve('candidate-emulator');
const root=path.join(source,'browser-output/wasm');
const runtime=path.join(root,'runtime');
const evidence=path.resolve('artifacts/pr36-idle-gc-gate');
const readJSON=async p=>JSON.parse(await fs.readFile(p,'utf8'));
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
await fs.mkdir(evidence,{recursive:true});
assert.equal(git(source,'rev-parse','HEAD'),baseline);
git(source,'diff','--exit-code','HEAD','--');
const sourceIdentity=await verifySource(candidate);
const build=await verifyBuild(candidate,buildDirectory);
const provenance=await readJSON(path.join(buildDirectory,'candidate-provenance.json'));
for (const key of ['baselineSource','candidateSource','changedFiles','sourceFiles']) assert.deepEqual(provenance[key],sourceIdentity[key]);
assert.equal(provenance.workflowCommit,process.env.GITHUB_SHA);
assert.equal(provenance.runId,Number(process.env.GITHUB_RUN_ID));
assert.equal(provenance.runAttempt,1);
assert.equal(process.env.GITHUB_RUN_ATTEMPT,'1');
assert.equal(provenance.browserAcceptanceVerified,false);
assert.deepEqual(provenance.build,await record(path.join(buildDirectory,'build.json')));
assert.deepEqual(provenance.files,build.files);
const archive=await readJSON('prepared/candidate-emulator-archive-provenance.json');
assert.equal(archive.run,provenance.runId);
assert.equal(archive.workflowCommit,provenance.workflowCommit);
assert.equal(archive.buildJob.conclusion,'success');
assert.equal(archive.buildJob.head_sha,provenance.workflowCommit);
assert.deepEqual(archive.members['build.json'],provenance.build);
assert.deepEqual(archive.members['candidate-provenance.json'],await record(path.join(buildDirectory,'candidate-provenance.json')));
const baselineProofPath='artifacts/openfga10-bfs-browser-gate/materialization.json';
const materialization=await readJSON(baselineProofPath);
assert.equal(materialization.sourceCommit,baseline);
assert.equal(materialization.rootfsUnchanged,true);
assert.equal(materialization.authorizationDeadlinesChanged,false);
const nativeInputs=(await readJSON('prepared/native-evidence/openfga10-verifier-input.json')).verifiedInputs;
assert.equal(Object.keys(nativeInputs).length,17);
for (const [name,expected] of Object.entries(nativeInputs)) {
  const file={bytes:expected.bytes,sha256:expected.sha256};
  assert.deepEqual(await record(path.join(source,name)),file,'Runtime native input differs: '+name);
  assert.deepEqual(await record(path.join(candidate,name)),file,'Candidate native input differs: '+name);
  assert.equal(git(source,'ls-tree','HEAD','--',name),git(candidate,'ls-tree','HEAD','--',name));
}
const allowed=name=>['runtime/out.js','runtime/qemu-system-x86_64.wasm','runtime/derived-runtime.json',
  'runtime/startup-manifest.json'].includes(name)||name.startsWith('runtime/emulator-build/');
async function preserved(directory,prefix='') {
  const records={};
  for (const item of await fs.readdir(directory,{withFileTypes:true})) {
    const name=prefix+item.name,filename=path.join(directory,item.name);
    assert(!item.isSymbolicLink(),'Unexpected runtime symlink');
    if(item.isDirectory()) Object.assign(records,await preserved(filename,name+'/'));
    else {assert(item.isFile(),'Unexpected runtime file');if(!allowed(name))records[name]=await record(filename);}
  }
  return records;
}
const before=await preserved(root);
assert.deepEqual(before['runtime/qemu-system-x86_64.data'],materialization.data);
assert.deepEqual(before['runtime/load.js'],materialization.loader);
assert.deepEqual(before['runtime/kernel-build/bzImage'],materialization.kernel);
assert.deepEqual(before['runtime/openfga'],materialization.openfgaBinary);
assert.deepEqual(before['runtime/bootstrap'],materialization.nativeVerification.bootstrapBinary);
const startupPath=path.join(runtime,'startup-manifest.json');
const derivedPath=path.join(runtime,'derived-runtime.json');
const originalStartup=await fs.readFile(startupPath);
assert.deepEqual(await record(startupPath),materialization.startupManifest);
const startup=JSON.parse(originalStartup);
const baselineEmulator=startup.emulator_build;
execFileSync(process.execPath,[path.join(candidate,'browser-single/prepare-emulator.mjs'),runtime,buildDirectory],
  {cwd:candidate,stdio:'inherit'});
assert.deepEqual(await fs.readFile(startupPath),originalStartup,'The pair overlay unexpectedly changed startup inputs');
startup.emulator_build=build;
const startupBytes=Buffer.from(JSON.stringify(startup,null,2)+'\n');
await fs.writeFile(startupPath,startupBytes);
const derived=await readJSON(derivedPath);
assert.deepEqual(derived.emulator_build,build);
assert.deepEqual(derived.data,materialization.data);
assert.equal(derived.kernel_sha256,materialization.kernel.sha256);
assert.equal(derived.loader_sha256,materialization.loader.sha256);
assert.equal(derived.rootfs_changed,false);
assert.equal(derived.startup_overlay.manifest,'startup-manifest.json');
derived.startup_overlay.sha256=sha(startupBytes);
await fs.writeFile(derivedPath,JSON.stringify(derived,null,2)+'\n');
assert.deepEqual(await preserved(root),before,'Emulator overlay modified an input outside its explicit delta');
for(const [name,expected] of Object.entries(build.files))assert.deepEqual(await record(path.join(runtime,name)),expected);
assert.deepEqual(await readJSON(path.join(runtime,'emulator-build/build.json')),build);
git(source,'diff','--exit-code','HEAD','--');
const proof={kind:'verified_emulator_only_overlay',baselineSource:baseline,sourceIdentity,
  candidateArtifact:archive.artifact,candidateArchive:archive.archive,producer:provenance,
  baselineMaterialization:await record(baselineProofPath),baselineEmulator,candidateEmulator:build,
  preservedFiles:before,nativeInputs,all17NativeInputsUnchanged:true,
  startupManifest:await record(startupPath),derivedRuntime:await record(derivedPath),
  materializer:await record(path.join(candidate,'browser-single/prepare-emulator.mjs')),
  vmConfiguration:materialization.expectedRuntimeConfiguration,authorizationDeadlinesChanged:false,
  browserAcceptanceVerified:false};
await fs.writeFile(path.join(evidence,'emulator-overlay.json'),JSON.stringify(proof,null,2)+'\n');
console.log(JSON.stringify({kind:proof.kind,candidateSource:sourceIdentity.candidateSource,
  backendPostimage:build.backend_postimage_blob,preservedFiles:Object.keys(before).length,
  all17NativeInputsUnchanged:true,browserAcceptanceVerified:false}));
