// Assemble a new concept scenario on the exact previously verified VM payload.
// The retained application/rootfs is unchanged; the new Rust executable and
// two-type model have separate current-run producer and native integration proof.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {baseline,git,record} from './retained/candidate-emulator.mjs';

const source=path.resolve('source-candidate');
const baselineRoot=path.join(source,'browser-output/wasm');
const output=path.resolve('browser-output/concept');
const nativeDirectory=path.resolve('artifacts/browser-concept-native');
const sourceCommit=git(process.cwd(),'rev-parse','HEAD');
const readJSON=async name=>JSON.parse(await fs.readFile(name,'utf8'));
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
const writeJSON=async (name,value)=>fs.writeFile(name,JSON.stringify(value,null,2)+'\n',{flag:'wx'});
assert.equal(sourceCommit,process.env.GITHUB_SHA);
assert.equal(process.env.GITHUB_RUN_ATTEMPT,'1');
assert.equal(process.env.BROWSER_CONCEPT_ENGINE,'chromium');
git(process.cwd(),'diff','--exit-code','HEAD','--');
assert.equal(git(source,'rev-parse','HEAD'),baseline);
git(source,'diff','--exit-code','HEAD','--');

const frozen=await readJSON('browser-concept/retained/provenance.json');
assert.equal(frozen.sourceCommit,'8abc70fc732af6955a9c1ce9473e5bca25229467');
for(const [name,expected] of Object.entries(frozen.unchangedFiles)) {
  assert.deepEqual(await record(path.join('browser-concept/retained',name)),
    {bytes:expected.bytes,sha256:expected.sha256},'Frozen runtime tooling changed: '+name);
}
const materializationPath='artifacts/openfga10-bfs-browser-gate/materialization.json';
const emulatorOverlayPath='artifacts/pr36-idle-gc-gate/emulator-overlay.json';
const nativeProofPath='artifacts/browser-concept-native-proof/result.json';
const materialization=await readJSON(materializationPath);
const emulatorOverlay=await readJSON(emulatorOverlayPath);
const conceptBuild=await readJSON(path.join(nativeDirectory,'build.json'));
const nativeProof=await readJSON(nativeProofPath);
const conceptArtifact=await readJSON('prepared/concept-native-archive-provenance.json');
assert.equal(materialization.sourceCommit,baseline);
assert.equal(materialization.originalRuntimeSource,'99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58');
assert.equal(materialization.rootfsUnchanged,true);
assert.equal(materialization.expectedRuntimeConfiguration.virtualCPUs,2);
assert.equal(materialization.expectedRuntimeConfiguration.memoryMiB,512);
assert.equal(emulatorOverlay.kind,'verified_emulator_only_overlay');
assert.equal(emulatorOverlay.sourceIdentity.candidateSource,'6f35f24b7ca345cf47c873becee6a41ef94e65a9');
assert.equal(emulatorOverlay.emulatorRebuilt,false);
assert.equal(emulatorOverlay.all17NativeInputsUnchanged,true);
assert.deepEqual(emulatorOverlay.baselineMaterialization,await record(materializationPath));
for(const [name,expected] of Object.entries(emulatorOverlay.preservedFiles)) {
  assert.deepEqual(await record(path.join(baselineRoot,name)),expected,'Retained runtime drift: '+name);
}
for(const [name,expected] of Object.entries(emulatorOverlay.candidateEmulator.files)) {
  assert.deepEqual(await record(path.join(baselineRoot,'runtime',name)),expected,'Retained emulator drift: '+name);
}
assert.deepEqual(await record(path.join(baselineRoot,'runtime/startup-manifest.json')),emulatorOverlay.startupManifest);
assert.deepEqual(await record(path.join(baselineRoot,'runtime/derived-runtime.json')),emulatorOverlay.derivedRuntime);
assert.equal(Object.keys(emulatorOverlay.nativeInputs).length,17);
for(const [name,expected] of Object.entries(emulatorOverlay.nativeInputs)) {
  assert.deepEqual(await record(path.join(source,name)),{bytes:expected.bytes,sha256:expected.sha256},'Retained OpenFGA input drift: '+name);
}
assert.equal(conceptBuild.schemaVersion,1);
assert.equal(conceptBuild.kind,'browser_concept_native');
assert.equal(conceptBuild.sourceCommit,sourceCommit);
assert.equal(conceptBuild.sourceTree,git(process.cwd(),'rev-parse','HEAD^{tree}'));
assert.equal(conceptBuild.target,'x86_64-unknown-linux-gnu');
assert.deepEqual(conceptBuild.binary,await record(path.join(nativeDirectory,'concept')));
assert.deepEqual(conceptBuild.model,await record('browser-concept/model.json'));
assert.deepEqual(conceptBuild.cargoToml,await record('Cargo.toml'));
assert.deepEqual(conceptBuild.cargoLock,await record('Cargo.lock'));
assert.deepEqual(conceptBuild.sourceManifest,await record(path.join(nativeDirectory,'source-manifest.json')));
assert.deepEqual(conceptArtifact.members['build.json'],await record(path.join(nativeDirectory,'build.json')));
assert.deepEqual(conceptArtifact.members.concept,conceptBuild.binary);
assert.equal(conceptArtifact.producer.runId,Number(process.env.GITHUB_RUN_ID));
assert.equal(conceptArtifact.producer.workflowCommit,sourceCommit);
for(const name of ['rustfmt','clippy','unitTests'])assert.equal(conceptBuild.checks[name].passed,true);
assert.equal(nativeProof.passed,true,'The real PostgreSQL/OpenFGA native integration must pass before assembly');
assert.equal(nativeProof.sourceCommit,sourceCommit);
assert.equal(nativeProof.runId,Number(process.env.GITHUB_RUN_ID));
assert.deepEqual(nativeProof.conceptBinary,conceptBuild.binary);
assert.deepEqual(nativeProof.model,conceptBuild.model);

async function inventory(directory,prefix='') {
  const files={};
  for(const item of await fs.readdir(directory,{withFileTypes:true})) {
    const name=prefix+item.name,filename=path.join(directory,item.name);
    assert(!item.isSymbolicLink(),'Unexpected runtime symlink: '+name);
    if(item.isDirectory())Object.assign(files,await inventory(filename,name+'/'));
    else {assert(item.isFile());files[name]=await record(filename);}
  }
  return files;
}
const originalRuntime=await inventory(path.join(baselineRoot,'runtime'));
await fs.mkdir(path.dirname(output),{recursive:true});
await fs.mkdir(output,{recursive:false});
await fs.cp(path.join(baselineRoot,'runtime'),path.join(output,'runtime'),{recursive:true,errorOnExist:true,force:false});
for(const name of ['index.html','site.css','launcher.js','protocol.mjs','startup-payload.mjs','isolation-sw.js']) {
  await fs.copyFile(path.join('browser-concept/pages',name),path.join(output,name));
}
const helperFiles={};
for(const name of ['log-stream.mjs','runtime-errors.mjs','qemu-budget.mjs']) {
  const filename=path.join(source,'browser-single/pages',name);
  await fs.copyFile(filename,path.join(output,name));
  helperFiles[name]=await record(filename);
  assert.deepEqual(await record(path.join(output,name)),helperFiles[name]);
}
for(const name of ['guest.sh','entrypoint.sh','model.json','serve.mjs']) {
  await fs.copyFile(path.join('browser-concept',name),path.join(output,name));
}
await fs.copyFile(path.join(nativeDirectory,'concept'),path.join(output,'runtime/concept'));
const payloadFiles=['guest.sh','entrypoint.sh','runtime/openfga','runtime/concept','model.json'];
const files={};
for(const name of payloadFiles)files[name]=await record(path.join(output,name));
assert.deepEqual(files['runtime/openfga'],materialization.openfgaBinary);
assert.deepEqual(files['runtime/concept'],conceptBuild.binary);
assert.deepEqual(files['model.json'],conceptBuild.model);
const requiredRuntime=['runtime/load.js','runtime/arg-module.js','runtime/out.js',
  'runtime/qemu-system-x86_64.wasm','runtime/qemu-system-x86_64.data'];
let downloadBytes=0;
for(const name of [...payloadFiles,...requiredRuntime])downloadBytes+=(await fs.stat(path.join(output,name))).size;
assert(Number.isSafeInteger(downloadBytes)&&downloadBytes>0);
const manifest={schemaVersion:1,kind:'browser_concept_startup',
  base_source_commit:materialization.originalRuntimeSource,runtimeSourceCommit:baseline,conceptSourceCommit:sourceCommit,
  model:conceptBuild.model,conceptBuild,
  nativeProof:{passed:true,record:await record(nativeProofPath)},
  retained:{openfgaBuild:materialization.openfgaBuild,emulatorBuild:emulatorOverlay.candidateEmulator,
    materialization:await record(materializationPath),emulatorOverlay:await record(emulatorOverlayPath)},
  files,downloadBytes};
const manifestBytes=Buffer.from(JSON.stringify(manifest,null,2)+'\n');
await fs.writeFile(path.join(output,'runtime/startup-manifest.json'),manifestBytes);
const derived=await readJSON(path.join(output,'runtime/derived-runtime.json'));
derived.startup_overlay={manifest:'startup-manifest.json',sha256:digest(manifestBytes)};
derived.concept_overlay={source_commit:sourceCommit,binary:conceptBuild.binary,model:conceptBuild.model,
  original_rootfs_preserved:true,postgres_application:true,openfga_datastore:'memory'};
await fs.writeFile(path.join(output,'runtime/derived-runtime.json'),JSON.stringify(derived,null,2)+'\n');
const afterRuntime=await inventory(path.join(output,'runtime'));
const changedRuntime=['startup-manifest.json','derived-runtime.json'];
for(const [name,expected] of Object.entries(originalRuntime)) {
  if(!changedRuntime.includes(name))assert.deepEqual(afterRuntime[name],expected,'Unrelated retained runtime changed: '+name);
}
assert.deepEqual(Object.keys(afterRuntime).filter(name=>!(name in originalRuntime)),['concept']);

const provenance=path.join(output,'provenance');
await fs.mkdir(provenance);
for(const [from,to] of [[materializationPath,'retained-materialization.json'],[emulatorOverlayPath,'retained-emulator-overlay.json'],
  [nativeProofPath,'native-proof.json'],[path.join(nativeDirectory,'build.json'),'concept-build.json'],
  ['prepared/concept-native-archive-provenance.json','concept-native-archive-provenance.json'],
  ['browser-concept/retained/provenance.json','frozen-tooling.json']]) {
  await fs.copyFile(from,path.join(provenance,to));
}
await fs.mkdir(path.join(provenance,'retained-inputs'));
const archiveRecords={};
for(const label of ['retained','kernel','native','bootstrap','lightweight','candidate-emulator']) {
  const filename=label+'-archive-provenance.json';
  await fs.copyFile(path.join('prepared',filename),path.join(provenance,'retained-inputs',filename));
  archiveRecords[label]=await record(path.join('prepared',filename));
}
await fs.mkdir(path.join(provenance,'native-build'));
for(const name of await fs.readdir(nativeDirectory)) {
  if(name!=='concept')await fs.copyFile(path.join(nativeDirectory,name),path.join(provenance,'native-build',name));
}
const siteFiles=await inventory(output);
const proof={schemaVersion:1,kind:'verified_browser_concept_assembly',conceptSourceCommit:sourceCommit,
  producerRunId:Number(process.env.GITHUB_RUN_ID),producerRunAttempt:1,
  retainedRuntimeSource:baseline,originalRootfsSource:materialization.originalRuntimeSource,
  emulatorSource:emulatorOverlay.sourceIdentity.candidateSource,
  originalMaterialization:await record(materializationPath),emulatorOverlay:await record(emulatorOverlayPath),
  conceptBuild:await record(path.join(nativeDirectory,'build.json')),conceptBinary:conceptBuild.binary,conceptModel:conceptBuild.model,
  nativeIntegrationProof:await record(nativeProofPath),nativeIntegrationPassed:true,
  originalRuntime,changedRetainedRuntimeFiles:changedRuntime,addedRuntimeFiles:['concept'],
  retainedOpenFGAInputs:emulatorOverlay.nativeInputs,all17RetainedOpenFGAInputsUnchanged:true,
  rootfsUnchanged:true,kernelUnchanged:true,emulatorUnchanged:true,
  helperFiles,originalArchiveRecords:archiveRecords,siteFiles,
  intendedConfiguration:{virtualCPUs:2,memoryMiB:512,postgresApplication:true,openfgaDatastore:'memory',
    application:'independent_two_type_document_concept',lakekeeperApplicationStarted:false},
  vmStarted:false,browserAcceptanceVerified:false,productionAcceptance:false};
await writeJSON(path.join(provenance,'assembly.json'),proof);
await fs.mkdir('artifacts/browser-concept-runtime',{recursive:true});
await writeJSON('artifacts/browser-concept-runtime/assembly.json',proof);
git(source,'diff','--exit-code','HEAD','--');
git(process.cwd(),'diff','--exit-code','HEAD','--');
console.log(JSON.stringify({kind:proof.kind,conceptSourceCommit:sourceCommit,conceptBinary:proof.conceptBinary,
  conceptModel:proof.conceptModel,downloadBytes,rootfsUnchanged:true,emulatorUnchanged:true,vmStarted:false}));
