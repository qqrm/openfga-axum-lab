// Apply the pinned guest topology delta only after the verified runtime/emulator overlays.
// The two-vCPU control retains every served byte; neither case edits source checkouts.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {baseline,git,record} from './candidate-emulator.mjs';

const cpuCandidateSource = 'e0e577d25103cd4d8013fcccafa8db4e3a8bd601';
assert(['1','2'].includes(process.env.BROWSER_DIAGNOSTIC_VCPUS),'Expected one or two guest vCPUs');
const cpus = Number(process.env.BROWSER_DIAGNOSTIC_VCPUS);
const source = path.resolve('source-candidate');
const candidate = path.resolve('cpu-source');
const root = path.join(source,'browser-output/wasm');
const evidence = path.resolve('artifacts/pr36-vcpu-gate');
const emulatorOverlayPath = 'artifacts/pr36-idle-gc-gate/emulator-overlay.json';
const materializationPath = 'artifacts/openfga10-bfs-browser-gate/materialization.json';
const readJSON = async filename => JSON.parse(await fs.readFile(filename,'utf8'));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const cpuPaths = ['browser-single/pages/guest.sh','browser-single/pages/launcher.js'];
const mutablePaths = ['guest.sh','launcher.js','runtime/startup-manifest.json','runtime/derived-runtime.json'];

assert.equal(git(source,'rev-parse','HEAD'),baseline);
assert.equal(git(candidate,'rev-parse','HEAD'),cpuCandidateSource);
assert.equal(git(candidate,'rev-parse','HEAD^'),baseline,'The topology candidate must have the exact runtime parent');
git(source,'diff','--exit-code','HEAD','--');
git(candidate,'diff','--exit-code','HEAD','--');
const changedFiles = git(candidate,'diff','--name-only',baseline,cpuCandidateSource,'--').split('\n').filter(Boolean).sort();
assert.deepEqual(changedFiles,cpuPaths,'The pinned topology delta changed files outside launcher and guest');
const sourceFiles = {};
for (const name of cpuPaths) {
  const versions = {};
  for (const [label,directory] of [['baseline',source],['candidate',candidate]]) {
    const entry = git(directory,'ls-tree','HEAD','--',name);
    const match = /^(100644|100755) blob ([0-9a-f]{40})\t/.exec(entry);
    assert(match,'Expected a regular topology source: '+name);
    assert.equal(git(directory,'hash-object','--',name),match[2]);
    versions[label] = {mode:match[1],gitBlob:match[2],...await record(path.join(directory,name))};
  }
  assert.equal(versions.baseline.mode,versions.candidate.mode,'Topology delta must not change source modes');
  sourceFiles[name] = versions;
}

const materialization = await readJSON(materializationPath);
const emulatorOverlay = await readJSON(emulatorOverlayPath);
assert.equal(materialization.sourceCommit,baseline);
assert.equal(materialization.rootfsUnchanged,true);
assert.equal(materialization.authorizationDeadlinesChanged,false);
assert.equal(materialization.expectedRuntimeConfiguration.virtualCPUs,2);
assert.equal(materialization.expectedRuntimeConfiguration.accel,'tcg,tb-size=500,thread=multi');
assert.equal(emulatorOverlay.kind,'verified_emulator_only_overlay');
assert.equal(emulatorOverlay.baselineSource,baseline);
assert.equal(emulatorOverlay.sourceIdentity.candidateSource,process.env.EMULATOR_SOURCE_SHA);
assert.equal(emulatorOverlay.all17NativeInputsUnchanged,true);
assert.equal(emulatorOverlay.authorizationDeadlinesChanged,false);
assert.equal(emulatorOverlay.browserAcceptanceVerified,false);
assert.deepEqual(emulatorOverlay.baselineMaterialization,await record(materializationPath));
assert.deepEqual(emulatorOverlay.vmConfiguration,materialization.expectedRuntimeConfiguration);
for (const [name,expected] of Object.entries(emulatorOverlay.preservedFiles)) {
  assert.deepEqual(await record(path.join(root,name)),expected,'Pre-topology runtime drift: '+name);
}
assert.deepEqual(await record(path.join(root,'runtime/startup-manifest.json')),emulatorOverlay.startupManifest);
assert.deepEqual(await record(path.join(root,'runtime/derived-runtime.json')),emulatorOverlay.derivedRuntime);
for (const [name,expected] of Object.entries(emulatorOverlay.candidateEmulator.files)) {
  assert.deepEqual(await record(path.join(root,'runtime',name)),expected,'Pre-topology emulator drift: '+name);
}

const nativeInputs = (await readJSON('prepared/native-evidence/openfga10-verifier-input.json')).verifiedInputs;
assert.equal(Object.keys(nativeInputs).length,17);
assert.deepEqual(nativeInputs,emulatorOverlay.nativeInputs);
for (const [name,expected] of Object.entries(nativeInputs)) {
  const expectedFile = {bytes:expected.bytes,sha256:expected.sha256};
  for (const directory of [source,candidate]) {
    assert.deepEqual(await record(path.join(directory,name)),expectedFile,'Topology changed a native input: '+name);
  }
  assert.equal(git(source,'ls-tree','HEAD','--',name),git(candidate,'ls-tree','HEAD','--',name));
}

async function preserved(directory,prefix='') {
  const files = {};
  for (const item of await fs.readdir(directory,{withFileTypes:true})) {
    const name = prefix+item.name;
    const filename = path.join(directory,item.name);
    assert(!item.isSymbolicLink(),'Unexpected served runtime symlink: '+name);
    if (item.isDirectory()) Object.assign(files,await preserved(filename,name+'/'));
    else {
      assert(item.isFile(),'Unexpected served runtime file: '+name);
      if (!mutablePaths.includes(name)) files[name] = await record(filename);
    }
  }
  return files;
}
const preservedFiles = await preserved(root);
for (const [name,expected] of Object.entries({
  'runtime/qemu-system-x86_64.data':materialization.data,
  'runtime/load.js':materialization.loader,
  'runtime/kernel-build/bzImage':materialization.kernel,
  'runtime/openfga':materialization.openfgaBinary,
  'runtime/bootstrap':materialization.nativeVerification.bootstrapBinary
})) assert.deepEqual(preservedFiles[name],expected,'Required preserved input differs: '+name);
const beforeFiles = {};
for (const name of mutablePaths) beforeFiles[name] = await record(path.join(root,name));
for (const name of cpuPaths) {
  const relative = path.basename(name);
  assert.deepEqual(beforeFiles[relative],materialization.sourceFiles[name]);
  assert.deepEqual(beforeFiles[relative],await record(path.join(source,name)));
}

const startupPath = path.join(root,'runtime/startup-manifest.json');
const derivedPath = path.join(root,'runtime/derived-runtime.json');
const originalStartup = await readJSON(startupPath);
const originalDerived = await readJSON(derivedPath);
assert.deepEqual(originalStartup.emulator_build,emulatorOverlay.candidateEmulator);
assert.deepEqual(originalStartup.files['guest.sh'],beforeFiles['guest.sh']);
assert.equal(originalDerived.startup_overlay.manifest,'startup-manifest.json');
assert.equal(originalDerived.startup_overlay.sha256,beforeFiles['runtime/startup-manifest.json'].sha256);
const expectedStartup = structuredClone(originalStartup);
const expectedDerived = structuredClone(originalDerived);
if (cpus===1) {
  for (const name of cpuPaths) await fs.copyFile(path.join(candidate,name),path.join(root,path.basename(name)));
  expectedStartup.files['guest.sh'] = await record(path.join(root,'guest.sh'));
  const startupBytes = Buffer.from(JSON.stringify(expectedStartup,null,2)+'\n');
  await fs.writeFile(startupPath,startupBytes);
  expectedDerived.startup_overlay.sha256 = digest(startupBytes);
  await fs.writeFile(derivedPath,JSON.stringify(expectedDerived,null,2)+'\n');
}

assert.deepEqual(await readJSON(startupPath),expectedStartup,'Topology overlay changed other startup fields');
assert.deepEqual(await readJSON(derivedPath),expectedDerived,'Topology overlay changed other derived metadata');
assert.deepEqual(await preserved(root),preservedFiles,'Topology overlay changed an unrelated served input');
const afterFiles = {};
for (const name of mutablePaths) afterFiles[name] = await record(path.join(root,name));
if (cpus===2) assert.deepEqual(afterFiles,beforeFiles,'Two-vCPU control must preserve all served bytes');
const selectedDirectory = cpus===1 ? candidate : source;
for (const name of cpuPaths) {
  assert.deepEqual(await fs.readFile(path.join(root,path.basename(name))),await fs.readFile(path.join(selectedDirectory,name)),
    'Served topology file differs from the pinned source: '+name);
}
git(source,'diff','--exit-code','HEAD','--');
git(candidate,'diff','--exit-code','HEAD','--');
const proof = {
  kind:'verified_guest_vcpu_topology_overlay',baselineSource:baseline,cpuCandidateSource,
  requestedVirtualCPUs:cpus,selectedSource:cpus===1 ? cpuCandidateSource : baseline,
  sourceIdentity:{parent:baseline,candidate:cpuCandidateSource,changedFiles,sourceFiles},
  emulatorOverlay:await record(emulatorOverlayPath),baselineMaterialization:await record(materializationPath),
  beforeFiles,afterFiles,preservedFiles,selectedLauncher:afterFiles['launcher.js'],selectedGuest:afterFiles['guest.sh'],
  startupManifest:afterFiles['runtime/startup-manifest.json'],derivedRuntime:afterFiles['runtime/derived-runtime.json'],
  nativeInputs,all17NativeInputsUnchanged:true,sourceCheckoutsUnchanged:true,
  controlBytesUnchanged:cpus===2,
  vmConfiguration:{...materialization.expectedRuntimeConfiguration,virtualCPUs:cpus,
    cpuTopology:cpus+',sockets='+cpus+',cores=1,threads=1',onlineCPUs:cpus===1 ? '0' : '0-1'},
  rootfsUnchanged:true,applicationBinariesUnchanged:true,kernelUnchanged:true,rngConfigurationUnchanged:true,
  authorizationDeadlinesChanged:false,browserAcceptanceVerified:false
};
await fs.mkdir(evidence,{recursive:true});
await fs.writeFile(path.join(evidence,'topology-overlay.json'),JSON.stringify(proof,null,2)+'\n');
console.log(JSON.stringify({kind:proof.kind,requestedVirtualCPUs:cpus,selectedSource:proof.selectedSource,
  cpuCandidateSource,preservedFiles:Object.keys(preservedFiles).length,all17NativeInputsUnchanged:true,
  controlBytesUnchanged:proof.controlBytesUnchanged,browserAcceptanceVerified:false}));
