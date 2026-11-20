// Reuse the exact frozen stack; replace only its guest orchestration for a component diagnostic.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {baseline,git,record} from './candidate-emulator.mjs';

const source = path.resolve('source-candidate');
const root = path.join(source,'browser-output/wasm');
const evidence = path.resolve('artifacts/pr36-memory-component');
const materializationPath = 'artifacts/openfga10-bfs-browser-gate/materialization.json';
const emulatorOverlayPath = 'artifacts/pr36-idle-gc-gate/emulator-overlay.json';
const componentScript = path.resolve('diagnostics/openfga-memory-guest.sh');
const mutable = ['guest.sh','runtime/startup-manifest.json','runtime/derived-runtime.json'];
const readJSON = async filename => JSON.parse(await fs.readFile(filename,'utf8'));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

assert.equal(git(source,'rev-parse','HEAD'),baseline);
git(source,'diff','--exit-code','HEAD','--');
const materialization = await readJSON(materializationPath);
const overlay = await readJSON(emulatorOverlayPath);
assert.equal(materialization.sourceCommit,baseline);
assert.equal(materialization.rootfsUnchanged,true);
assert.equal(materialization.authorizationDeadlinesChanged,false);
assert.equal(materialization.expectedRuntimeConfiguration.virtualCPUs,2);
assert.equal(materialization.expectedRuntimeConfiguration.memoryMiB,512);
assert.equal(materialization.expectedRuntimeConfiguration.accel,'tcg,tb-size=500,thread=multi');
assert.equal(overlay.kind,'verified_emulator_only_overlay');
assert.equal(overlay.baselineSource,baseline);
assert.equal(overlay.sourceIdentity.candidateSource,process.env.EMULATOR_SOURCE_SHA);
assert.equal(overlay.all17NativeInputsUnchanged,true);
assert.equal(overlay.authorizationDeadlinesChanged,false);
assert.equal(overlay.browserAcceptanceVerified,false);
assert.deepEqual(overlay.baselineMaterialization,await record(materializationPath));
assert.deepEqual(overlay.vmConfiguration,materialization.expectedRuntimeConfiguration);
for (const [name,expected] of Object.entries(overlay.preservedFiles)) {
  assert.deepEqual(await record(path.join(root,name)),expected,'Pre-component runtime drift: '+name);
}
for (const [name,expected] of Object.entries(overlay.candidateEmulator.files)) {
  assert.deepEqual(await record(path.join(root,'runtime',name)),expected,'Pre-component emulator drift: '+name);
}
assert.deepEqual(await record(path.join(root,'runtime/startup-manifest.json')),overlay.startupManifest);
assert.deepEqual(await record(path.join(root,'runtime/derived-runtime.json')),overlay.derivedRuntime);
assert.deepEqual(await record(path.join(root,'launcher.js')),materialization.sourceFiles['browser-single/pages/launcher.js']);
assert.deepEqual(await record(path.join(root,'guest.sh')),materialization.configurationExperiment.guest);

const nativeInputs = (await readJSON('prepared/native-evidence/openfga10-verifier-input.json')).verifiedInputs;
assert.equal(Object.keys(nativeInputs).length,17);
assert.deepEqual(nativeInputs,overlay.nativeInputs);
for (const [name,expected] of Object.entries(nativeInputs)) {
  assert.deepEqual(await record(path.join(source,name)),{bytes:expected.bytes,sha256:expected.sha256},
    'The component diagnostic changed a native producer input: '+name);
}
async function preserved(directory,prefix='') {
  const records = {};
  for (const item of await fs.readdir(directory,{withFileTypes:true})) {
    const name = prefix+item.name;
    const filename = path.join(directory,item.name);
    assert(!item.isSymbolicLink(),'Unexpected served runtime symlink: '+name);
    if (item.isDirectory()) Object.assign(records,await preserved(filename,name+'/'));
    else {
      assert(item.isFile(),'Unexpected served runtime file: '+name);
      if (!mutable.includes(name)) records[name] = await record(filename);
    }
  }
  return records;
}
const preservedFiles = await preserved(root);
for (const [name,expected] of Object.entries({
  'runtime/qemu-system-x86_64.data':materialization.data,
  'runtime/load.js':materialization.loader,
  'runtime/kernel-build/bzImage':materialization.kernel,
  'runtime/openfga':materialization.openfgaBinary,
  'runtime/bootstrap':materialization.nativeVerification.bootstrapBinary,
  'entrypoint.sh':materialization.sourceFiles['browser-single/entrypoint.sh']
})) assert.deepEqual(preservedFiles[name],expected,'Required frozen input differs: '+name);
const beforeFiles = {};
for (const name of mutable) beforeFiles[name] = await record(path.join(root,name));
const startupPath = path.join(root,'runtime/startup-manifest.json');
const derivedPath = path.join(root,'runtime/derived-runtime.json');
const originalStartup = await readJSON(startupPath);
const originalDerived = await readJSON(derivedPath);
assert.deepEqual(originalStartup.emulator_build,overlay.candidateEmulator);
assert.deepEqual(originalStartup.files['guest.sh'],beforeFiles['guest.sh']);
assert.equal(originalDerived.startup_overlay.manifest,'startup-manifest.json');
assert.equal(originalDerived.startup_overlay.sha256,beforeFiles['runtime/startup-manifest.json'].sha256);
await fs.copyFile(componentScript,path.join(root,'guest.sh'));
const componentGuest = await record(path.join(root,'guest.sh'));
assert.deepEqual(componentGuest,await record(componentScript));
const expectedStartup = structuredClone(originalStartup);
expectedStartup.files['guest.sh'] = componentGuest;
const startupBytes = Buffer.from(JSON.stringify(expectedStartup,null,2)+'\n');
await fs.writeFile(startupPath,startupBytes);
const expectedDerived = structuredClone(originalDerived);
expectedDerived.startup_overlay.sha256 = digest(startupBytes);
await fs.writeFile(derivedPath,JSON.stringify(expectedDerived,null,2)+'\n');
assert.deepEqual(await readJSON(startupPath),expectedStartup,'Unexpected startup manifest change');
assert.deepEqual(await readJSON(derivedPath),expectedDerived,'Unexpected derived metadata change');
assert.deepEqual(await preserved(root),preservedFiles,'Component overlay changed unrelated served bytes');
const afterFiles = {};
for (const name of mutable) afterFiles[name] = await record(path.join(root,name));
git(source,'diff','--exit-code','HEAD','--');
const proof = {
  kind:'verified_openfga_memory_component_overlay',baselineSource:baseline,
  emulatorSource:overlay.sourceIdentity.candidateSource,
  baselineMaterialization:await record(materializationPath),emulatorOverlay:await record(emulatorOverlayPath),
  sourceScript:{path:'diagnostics/openfga-memory-guest.sh',...await record(componentScript)},
  beforeFiles,afterFiles,preservedFiles,originalGuest:beforeFiles['guest.sh'],componentGuest,
  startupManifest:afterFiles['runtime/startup-manifest.json'],derivedRuntime:afterFiles['runtime/derived-runtime.json'],
  nativeInputs,all17NativeInputsUnchanged:true,applicationBinariesUnchanged:true,rootfsUnchanged:true,
  kernelUnchanged:true,rngConfigurationUnchanged:true,sourceCheckoutUnchanged:true,
  authorizationDeadlinesChanged:false,productionAcceptance:false,componentOnly:true,
  componentExecutionVerified:false,
  intendedConfiguration:{...materialization.expectedRuntimeConfiguration,datastoreEngine:'memory',
    postgres:false,axum:false,openfgaMigrate:false,listenerStartupSeconds:180,
    bootstrapModelTimeoutSeconds:30,ordinaryClientTimeoutSeconds:5,openfgaRequestTimeoutSeconds:3,
    cycles:2,requestsPerCycle:5,relation:'ownership',protocolPrefix:'FGA_MEMORY'},
  clientScope:'A bounded Bash HTTP/1.0 transport; actual HTTP and JSON results are verified by the host. It is not the Rust app/client acceptance.',
};
await fs.mkdir(evidence,{recursive:true});
await fs.writeFile(path.join(evidence,'component-overlay.json'),JSON.stringify(proof,null,2)+'\n');
console.log(JSON.stringify({kind:proof.kind,baselineSource:baseline,emulatorSource:proof.emulatorSource,
  preservedFiles:Object.keys(preservedFiles).length,all17NativeInputsUnchanged:true,componentOnly:true,
  productionAcceptance:false,componentExecutionVerified:false}));
