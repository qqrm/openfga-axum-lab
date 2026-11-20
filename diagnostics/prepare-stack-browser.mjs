// Reuse the frozen runtime; change only FGA datastore selection and observation.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {baseline,git,record} from './candidate-emulator.mjs';
import {transformStackRuntime} from './stack-runtime-transform.mjs';

const datastore=process.env.BROWSER_STACK_DATASTORE;
assert(['memory','postgres'].includes(datastore),'Select one declared FGA datastore');
assert.equal(process.env.BROWSER_DIAGNOSTIC_ENGINE,'chromium');
assert.equal(process.env.BROWSER_DIAGNOSTIC_VCPUS,'2');
assert.deepEqual(process.argv.slice(2),process.argv.includes('--restore')?['--restore']:[]);
const source=path.resolve('source-candidate');
const root=path.join(source,'browser-output/wasm');
const evidence=path.resolve('artifacts/pr36-stack-continuation',datastore);
const backup=path.resolve('prepared/stack-baseline');
const mutable=['guest.sh','entrypoint.sh','runtime/startup-manifest.json','runtime/derived-runtime.json'];
const readJSON=async file=>JSON.parse(await fs.readFile(file,'utf8'));
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
const materializationPath='artifacts/openfga10-bfs-browser-gate/materialization.json';
const emulatorPath='artifacts/pr36-idle-gc-gate/emulator-overlay.json';
const topologyPath='artifacts/pr36-vcpu-gate/topology-overlay.json';
assert.equal(git(source,'rev-parse','HEAD'),baseline);
git(source,'diff','--exit-code','HEAD','--');

async function inventory(directory,prefix='') {
  const files={};
  for(const item of await fs.readdir(directory,{withFileTypes:true})) {
    const name=prefix+item.name,filename=path.join(directory,item.name);
    assert(!item.isSymbolicLink(),'Unexpected served symlink: '+name);
    if(item.isDirectory()) Object.assign(files,await inventory(filename,name+'/'));
    else {assert(item.isFile());if(!mutable.includes(name))files[name]=await record(filename);}
  }
  return files;
}

if(process.argv.includes('--restore')) {
  assert.equal(datastore,'postgres','Restore only between the two declared scenarios');
  const previous=await readJSON('artifacts/pr36-stack-continuation/memory/stack-overlay.json');
  const summary=await readJSON('artifacts/pr36-stack-continuation/memory/summary.json');
  assert.equal(summary.checkerExited,true,'The first checker must have exited before restoring served bytes');
  assert.equal(summary.comparisonContinuationPermitted,true,'First capture does not permit the paired scenario');
  assert.equal(previous.baselineSource,baseline);
  assert.equal(previous.datastoreEngine,'memory');
  assert.deepEqual(await inventory(root),previous.preservedFiles);
  for(const [name,expected] of Object.entries(previous.afterFiles)) {
    assert.deepEqual(await record(path.join(root,name)),expected,'Current first-scenario bytes changed: '+name);
  }
  const original=await readJSON(path.join(backup,'manifest.json'));
  assert.equal(original.baselineSource,baseline);
  assert.deepEqual(original.files,previous.beforeFiles);
  for(const name of mutable) {
    assert.deepEqual(await record(path.join(backup,name)),original.files[name]);
    await fs.copyFile(path.join(backup,name),path.join(root,name));
    assert.deepEqual(await record(path.join(root,name)),original.files[name]);
  }
  assert.deepEqual(await inventory(root),previous.preservedFiles);
  await fs.mkdir(evidence,{recursive:true});
  await fs.writeFile(path.join(evidence,'baseline-restoration.json'),JSON.stringify({
    kind:'verified_between_closed_stack_checkers',baselineSource:baseline,
    firstCheckerSummary:await record('artifacts/pr36-stack-continuation/memory/summary.json'),
    originalFiles:original.files,restored:true,preservedFilesUnchanged:true,
  },null,2)+'\n');
  console.log('STACK_OVERLAY '+JSON.stringify({phase:'restored',datastore,files:mutable.length}));
} else {
  const materialization=await readJSON(materializationPath);
  const emulator=await readJSON(emulatorPath);
  const topology=await readJSON(topologyPath);
  assert.equal(materialization.sourceCommit,baseline);
  assert.equal(materialization.rootfsUnchanged,true);
  assert.equal(materialization.authorizationDeadlinesChanged,false);
  assert.equal(materialization.expectedRuntimeConfiguration.virtualCPUs,2);
  assert.equal(materialization.expectedRuntimeConfiguration.memoryMiB,512);
  assert.equal(materialization.expectedRuntimeConfiguration.accel,'tcg,tb-size=500,thread=multi');
  assert.equal(emulator.kind,'verified_emulator_only_overlay');
  assert.equal(emulator.baselineSource,baseline);
  assert.equal(emulator.sourceIdentity.candidateSource,process.env.EMULATOR_SOURCE_SHA);
  assert.equal(emulator.all17NativeInputsUnchanged,true);
  assert.equal(emulator.authorizationDeadlinesChanged,false);
  assert.deepEqual(emulator.baselineMaterialization,await record(materializationPath));
  assert.equal(topology.kind,'verified_guest_vcpu_topology_overlay');
  assert.equal(topology.requestedVirtualCPUs,2);
  assert.equal(topology.controlBytesUnchanged,true);
  assert.deepEqual(topology.emulatorOverlay,await record(emulatorPath));
  for(const [name,expected] of Object.entries(emulator.preservedFiles)) {
    assert.deepEqual(await record(path.join(root,name)),expected,'Baseline runtime drift: '+name);
  }
  for(const [name,expected] of Object.entries(emulator.candidateEmulator.files)) {
    assert.deepEqual(await record(path.join(root,'runtime',name)),expected,'Emulator drift: '+name);
  }
  assert.deepEqual(await record(path.join(root,'runtime/startup-manifest.json')),emulator.startupManifest);
  assert.deepEqual(await record(path.join(root,'runtime/derived-runtime.json')),emulator.derivedRuntime);
  assert.deepEqual(await record(path.join(root,'launcher.js')),materialization.sourceFiles['browser-single/pages/launcher.js']);
  assert.deepEqual(await record(path.join(root,'guest.sh')),materialization.configurationExperiment.guest);
  assert.deepEqual(await record(path.join(root,'entrypoint.sh')),materialization.sourceFiles['browser-single/entrypoint.sh']);
  const nativeInputs=(await readJSON('prepared/native-evidence/openfga10-verifier-input.json')).verifiedInputs;
  assert.equal(Object.keys(nativeInputs).length,17);
  assert.deepEqual(nativeInputs,emulator.nativeInputs);
  for(const [name,expected] of Object.entries(nativeInputs)) {
    assert.deepEqual(await record(path.join(source,name)),{bytes:expected.bytes,sha256:expected.sha256},'Changed native input: '+name);
  }
  const preservedFiles=await inventory(root),beforeFiles={};
  for(const name of mutable)beforeFiles[name]=await record(path.join(root,name));
  if(datastore==='memory') {
    await fs.mkdir(backup,{recursive:false});
    await fs.mkdir(path.join(backup,'runtime'));
    for(const name of mutable)await fs.copyFile(path.join(root,name),path.join(backup,name));
    await fs.writeFile(path.join(backup,'manifest.json'),JSON.stringify({baselineSource:baseline,files:beforeFiles},null,2)+'\n',{flag:'wx'});
  } else {
    const restored=await readJSON(path.join(evidence,'baseline-restoration.json'));
    assert.equal(restored.restored,true);
    assert.deepEqual(restored.originalFiles,beforeFiles);
  }
  const originalStartup=await readJSON(path.join(root,'runtime/startup-manifest.json'));
  const originalDerived=await readJSON(path.join(root,'runtime/derived-runtime.json'));
  assert.deepEqual(originalStartup.emulator_build,emulator.candidateEmulator);
  for(const name of ['guest.sh','entrypoint.sh'])assert.deepEqual(originalStartup.files[name],beforeFiles[name]);
  assert.equal(originalDerived.startup_overlay.manifest,'startup-manifest.json');
  assert.equal(originalDerived.startup_overlay.sha256,beforeFiles['runtime/startup-manifest.json'].sha256);
  const transformed=transformStackRuntime({
    guest:await fs.readFile(path.join(root,'guest.sh'),'utf8'),
    entrypoint:await fs.readFile(path.join(root,'entrypoint.sh'),'utf8'),datastore,
  });
  await fs.writeFile(path.join(root,'guest.sh'),transformed.guest);
  await fs.writeFile(path.join(root,'entrypoint.sh'),transformed.entrypoint);
  const expectedStartup=structuredClone(originalStartup);
  for(const name of ['guest.sh','entrypoint.sh'])expectedStartup.files[name]=await record(path.join(root,name));
  const startupBytes=Buffer.from(JSON.stringify(expectedStartup,null,2)+'\n');
  await fs.writeFile(path.join(root,'runtime/startup-manifest.json'),startupBytes);
  const expectedDerived=structuredClone(originalDerived);
  expectedDerived.startup_overlay.sha256=digest(startupBytes);
  await fs.writeFile(path.join(root,'runtime/derived-runtime.json'),JSON.stringify(expectedDerived,null,2)+'\n');
  assert.deepEqual(await inventory(root),preservedFiles,'Unrelated served bytes changed');
  const afterFiles={};for(const name of mutable)afterFiles[name]=await record(path.join(root,name));
  assert.deepEqual(await readJSON(path.join(root,'runtime/startup-manifest.json')),expectedStartup);
  assert.deepEqual(await readJSON(path.join(root,'runtime/derived-runtime.json')),expectedDerived);
  git(source,'diff','--exit-code','HEAD','--');
  const proof={kind:'verified_full_stack_datastore_overlay',baselineSource:baseline,
    emulatorSource:emulator.sourceIdentity.candidateSource,datastoreEngine:datastore,
    baselineMaterialization:await record(materializationPath),emulatorOverlay:await record(emulatorPath),
    topologyOverlay:await record(topologyPath),transform:transformed.proof,
    transformImplementation:await record('diagnostics/stack-runtime-transform.mjs'),
    beforeFiles,afterFiles,preservedFiles,nativeInputs,
    startupManifest:afterFiles['runtime/startup-manifest.json'],derivedRuntime:afterFiles['runtime/derived-runtime.json'],
    all17NativeInputsUnchanged:true,applicationBinariesUnchanged:true,rootfsUnchanged:true,kernelUnchanged:true,
    rngConfigurationUnchanged:true,sourceCheckoutsUnchanged:true,sourceCheckoutUnchanged:true,
    originalCheckerUnchanged:true,originalAcceptanceUnchanged:true,authorizationDeadlinesChanged:false,
    productionAcceptance:false,applicationAcceptanceVerified:false,
    intendedConfiguration:{...materialization.expectedRuntimeConfiguration,datastoreEngine:datastore,
      postgresForApplication:true,axum:true,openfgaMigrate:datastore==='postgres',
      originalAcceptanceRuns:2,firstWaitMilliseconds:1200000,secondWaitMilliseconds:180000},
  };
  await fs.mkdir(evidence,{recursive:true});
  await fs.writeFile(path.join(evidence,'stack-overlay.json'),JSON.stringify(proof,null,2)+'\n');
  console.log('STACK_OVERLAY '+JSON.stringify({phase:'prepared',datastore,all17NativeInputsUnchanged:true,applicationAcceptanceVerified:false}));
}
