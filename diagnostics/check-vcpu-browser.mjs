// Invoke the exact production checker once for one engine and guest CPU count.
// Record actual topology independently of acceptance, including failed boots.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
import {record,verifyBuild} from './candidate-emulator.mjs';

const sourceCommit = '444b069daf16e9606a0bbf1ea0aea7f4d61b97d8';
const cpuCandidateSource = 'e0e577d25103cd4d8013fcccafa8db4e3a8bd601';
const engine = process.env.BROWSER_DIAGNOSTIC_ENGINE;
assert(['chromium','firefox'].includes(engine),'Expected one allowlisted engine');
assert(['1','2'].includes(process.env.BROWSER_DIAGNOSTIC_VCPUS),'Expected one or two guest vCPUs');
const cpus = Number(process.env.BROWSER_DIAGNOSTIC_VCPUS);
const expectedTopology = cpus+',sockets='+cpus+',cores=1,threads=1';
const expectedCPUList = cpus===1 ? '0' : '0-1';
const source = path.resolve('source-candidate');
const evidenceDirectory = path.resolve('artifacts/pr36-vcpu-gate');
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const materialization = JSON.parse(await fs.readFile('artifacts/openfga10-bfs-browser-gate/materialization.json','utf8'));
const emulatorOverlayPath = 'artifacts/pr36-idle-gc-gate/emulator-overlay.json';
const overlay = JSON.parse(await fs.readFile(emulatorOverlayPath,'utf8'));
const topology = JSON.parse(await fs.readFile(path.join(evidenceDirectory,'topology-overlay.json'),'utf8'));
assert.equal(overlay.kind,'verified_emulator_only_overlay');
assert.equal(overlay.sourceIdentity.candidateSource,process.env.EMULATOR_SOURCE_SHA);
assert.equal(overlay.all17NativeInputsUnchanged,true);
assert.equal(overlay.authorizationDeadlinesChanged,false);
assert.equal(overlay.browserAcceptanceVerified,false);
assert.equal(topology.kind,'verified_guest_vcpu_topology_overlay');
assert.equal(topology.baselineSource,sourceCommit);
assert.equal(topology.cpuCandidateSource,cpuCandidateSource);
assert.equal(topology.requestedVirtualCPUs,cpus);
assert.equal(topology.selectedSource,cpus===1 ? cpuCandidateSource : sourceCommit);
assert.equal(topology.all17NativeInputsUnchanged,true);
assert.equal(topology.sourceCheckoutsUnchanged,true);
assert.equal(topology.rootfsUnchanged,true);
assert.equal(topology.applicationBinariesUnchanged,true);
assert.equal(topology.kernelUnchanged,true);
assert.equal(topology.rngConfigurationUnchanged,true);
assert.equal(topology.authorizationDeadlinesChanged,false);
assert.equal(topology.browserAcceptanceVerified,false);
assert.equal(topology.controlBytesUnchanged,cpus===2);
assert.equal(topology.vmConfiguration.virtualCPUs,cpus);
assert.equal(topology.vmConfiguration.cpuTopology,expectedTopology);
assert.equal(topology.vmConfiguration.onlineCPUs,expectedCPUList);
assert.deepEqual(topology.emulatorOverlay,await record(emulatorOverlayPath));
assert.deepEqual(topology.baselineMaterialization,await record('artifacts/openfga10-bfs-browser-gate/materialization.json'));
assert.deepEqual(topology.nativeInputs,overlay.nativeInputs);
const candidateBuild = await verifyBuild(path.resolve('emulator-source'),path.resolve('candidate-emulator'));
assert.deepEqual(candidateBuild,overlay.candidateEmulator);
const wasmRoot = path.join(source,'browser-output/wasm');
for (const [name,expected] of Object.entries(overlay.preservedFiles)) {
  if (!['launcher.js','guest.sh'].includes(name)) assert.deepEqual(await record(path.join(wasmRoot,name)),expected);
}
for (const [name,expected] of Object.entries(topology.preservedFiles)) assert.deepEqual(await record(path.join(wasmRoot,name)),expected);
for (const [name,expected] of Object.entries(topology.afterFiles)) assert.deepEqual(await record(path.join(wasmRoot,name)),expected);
for (const [name,expected] of Object.entries(candidateBuild.files)) assert.deepEqual(await record(path.join(wasmRoot,'runtime',name)),expected);
assert.deepEqual(await record(path.join(wasmRoot,'runtime/startup-manifest.json')),topology.startupManifest);
assert.deepEqual(await record(path.join(wasmRoot,'runtime/derived-runtime.json')),topology.derivedRuntime);
const selectedSource = cpus===1 ? path.resolve('cpu-source') : source;
const selectedHead = spawnSync('git',['rev-parse','HEAD'],{cwd:selectedSource,encoding:'utf8'});
assert.equal(selectedHead.status,0);
assert.equal(selectedHead.stdout.trim(),topology.selectedSource);
for (const name of ['guest.sh','launcher.js']) {
  assert.deepEqual(await fs.readFile(path.join(wasmRoot,name)),await fs.readFile(path.join(selectedSource,'browser-single/pages',name)));
}
const chromeSmoke = engine==='chromium'
  ? JSON.parse(await fs.readFile(path.join(evidenceDirectory,'chromium/system-chrome-smoke.json'),'utf8')) : null;
if (chromeSmoke) {
  assert.equal(chromeSmoke.passed,true);
  assert.equal(chromeSmoke.launchOptions.chromiumSandbox,true);
  assert.equal(chromeSmoke.launchOptions.channel,'chrome');
  assert.deepEqual(await record(chromeSmoke.executable.path),
    {bytes:chromeSmoke.executable.bytes,sha256:chromeSmoke.executable.sha256});
}

assert.equal(materialization.sourceCommit,sourceCommit);
assert.equal(materialization.configurationExperiment.bootstrap,'one_real_higher_consistency_ownership_deny');
assert.equal(materialization.configurationExperiment.kernelIRQ,'default_acpi');
assert.equal(materialization.configurationExperiment.runtimeSourceMatchesNativeProducer,false);
assert.equal(materialization.configurationExperiment.launcherIdentityVerified,true);
assert.equal(materialization.configurationExperiment.nativeProducerCommit,'bb8fed250bdee9931f7702ae2475d28e5e9eed8f');
assert.equal(materialization.configurationExperiment.guestDiagnosticInverseVerified,true);
assert.equal(materialization.configurationExperiment.launcherInverseVerified,true);
assert.equal(materialization.configurationExperiment.synchronousProtocolUnchanged,true);
assert.equal(materialization.configurationExperiment.nativeRuntimeDifferencePathsVerified,true);
assert.equal(materialization.nativeVerification.freshGoBuild.kind,'fresh_go_build');
assert.equal(materialization.nativeVerification.freshGoBuild.runtimeSource,sourceCommit);
assert.equal(materialization.nativeVerification.freshGoBuild.producerSource,'bb8fed250bdee9931f7702ae2475d28e5e9eed8f');
assert.equal(materialization.nativeVerification.freshGoBuild.run,43500949883);
assert.equal(materialization.nativeVerification.freshGoBuild.existingGoRegressionPackagesPassed,true);
assert.equal(materialization.openfgaBuild.schema_reachability_reuse,'readonly_plain_user_paths_on_startup_typesystem');
assert.equal(materialization.openfgaBuild.schema_reachability_max_relations,512);
assert.equal(materialization.openfgaBuild.schema_reachability_tests,'passed');
assert.equal(materialization.lightweightVerification.sourceCommit,'037a3c22a5fab45a735a6bfea9792c68049a4b62');
assert.equal(materialization.lightweightVerification.appliedRuntimeSource,sourceCommit);
assert.equal(materialization.lightweightVerification.launcherRngInverseVerified,true);
assert.equal(materialization.configurationExperiment.launcherRngInverseVerified,true);
assert.equal(materialization.configurationExperiment.kernelConfigOneFlagVerified,true);
assert.equal(materialization.configurationExperiment.entropyDeliveryVerified,false);
assert.equal(materialization.kernelVerification.sourceCommit,'716b77356dd4aab2747d4e37c5da723f3b7a764f');
assert.equal(materialization.kernelVerification.exactlyOneConfigChange,true);
assert.equal(materialization.kernelVerification.entropyDeliveryVerified,false);
assert.equal(materialization.lightweightVerification.passed,true);
assert.equal(materialization.lightweightVerification.productionAcceptance,false);
assert.equal(materialization.lightweightVerification.vmStarts,0);
const head = spawnSync('git',['rev-parse','HEAD'],{cwd:source,encoding:'utf8'});
assert.equal(head.status,0);
assert.equal(head.stdout.trim(),sourceCommit);
const originalPath = path.join(source,'browser-single/check-pages.mjs');
const original = await fs.readFile(originalPath,'utf8');
const initializer = "for (const [name, engine] of [['chromium',chromium],['firefox',firefox]]) {";
const singleton = engine==='chromium'
  ? "for (const [name, engine] of [['chromium',chromium]]) {"
  : "for (const [name, engine] of [['firefox',firefox]]) {";
assert.equal(original.split(initializer).length,2,'Production checker loop changed');
const originalLaunch = 'browser = await engine.launch({headless:true});';
const candidateLaunch = "browser = await engine.launch({headless:true, ...(name==='chromium' ? {channel:'chrome',chromiumSandbox:true,args:['--enable-automation']} : {})});";
assert.equal(original.split(originalLaunch).length,2,'Production browser launch changed');
const transformed = original.replace(initializer,()=>singleton).replace(originalLaunch,()=>candidateLaunch);
assert.equal(transformed.split(singleton).length,2);
assert.equal(transformed.split(candidateLaunch).length,2);
assert.deepEqual(Buffer.from(transformed.replace(singleton,()=>initializer).replace(candidateLaunch,()=>originalLaunch)),Buffer.from(original),
  'Only the engine selection and explicit Chromium sandbox/channel may change');
assert.equal(digest(original),materialization.sourceFiles['browser-single/check-pages.mjs'].sha256);
const copiedPath = path.join(source,'browser-single/check-pages.singleton-'+engine+'.mjs');
await fs.writeFile(copiedPath,transformed,{flag:'wx'});
const syntax = spawnSync(process.execPath,['--check',copiedPath],{cwd:source,stdio:'inherit'});
assert.equal(syntax.status,0,'Singleton checker syntax failed');
const transformProof = {
  sourceCommit,engine,requestedVirtualCPUs:cpus,original:{bytes:Buffer.byteLength(original),sha256:digest(original)},
  transformed:{bytes:Buffer.byteLength(transformed),sha256:digest(transformed)},
  copiedRelativePath:path.relative(source,copiedPath),inverseByteComparison:true,
  onlyChanges:['literal engine loop initializer','explicit system Chrome channel with sandbox'],maxVmStarts:1,
  firstWaitMilliseconds:1200000,secondWaitMilliseconds:180000,
  modelExplorerBuildVerified:false,diagnosticOnly:true
};
await fs.writeFile(path.join(evidenceDirectory,engine+'-checker-transform.json'),JSON.stringify(transformProof,null,2)+'\n');
// Use the current Actions job's actual start, including checkout/preflight.
// A late local timestamp must never manufacture another 30-minute budget.
const jobTiming = JSON.parse(await fs.readFile(path.resolve('prepared/job-timing.json'),'utf8'));
assert.equal(jobTiming.source,'GitHub Actions current job API');
assert.equal(jobTiming.name,'vCPU browser acceptance ('+engine+', '+cpus+' CPU)');
assert.equal(jobTiming.run_id,Number(process.env.GITHUB_RUN_ID));
assert.equal(jobTiming.run_attempt,1);
assert.equal(process.env.GITHUB_RUN_ATTEMPT,'1');
assert.equal(jobTiming.head_sha,process.env.GITHUB_SHA);
assert(Number.isSafeInteger(jobTiming.id) && jobTiming.id>0);
const checkedAt = Date.now();
const jobStartedAt = Date.parse(jobTiming.started_at);
const elapsed = checkedAt-jobStartedAt;
const jobBudget = {
  ...jobTiming,checkedAt:new Date(checkedAt).toISOString(),
  jobElapsedMilliseconds:Number.isFinite(elapsed)?elapsed:null,
  latestLaunchJobElapsedMilliseconds:240000,outerJobMinutes:30,runtimeStepMinutes:25,
  launchPermitted:Number.isFinite(elapsed) && elapsed>=0 && elapsed<=240000
};
await fs.writeFile(path.join(evidenceDirectory,engine+'-job-budget.json'),JSON.stringify(jobBudget,null,2)+'\n');
if (!jobBudget.launchPermitted) {
  const stopped = {phase:'preflight_stop',engine,requestedVirtualCPUs:cpus,sourceCommit,checkerLaunched:false,
    passes:null,reason:'Insufficient or unverified remaining Actions job budget',jobBudget};
  await fs.writeFile(path.join(evidenceDirectory,engine+'-preflight-stop.json'),JSON.stringify(stopped,null,2)+'\n');
  console.log('VCPU_BROWSER_GATE '+JSON.stringify(stopped));
  throw new Error('Refusing to start a VM more than four minutes after the Actions job began');
}
console.log('VCPU_BROWSER_GATE '+JSON.stringify({phase:'checker_launch',...transformProof,jobBudget}));
const startedAt = new Date().toISOString();
const heartbeat = setInterval(()=>console.log('VCPU_BROWSER_GATE '+JSON.stringify({phase:'acceptance_running',engine,requestedVirtualCPUs:cpus,
  elapsedMs:Date.now()-Date.parse(startedAt),candidateSource:overlay.sourceIdentity.candidateSource})),60000);
const child = spawn(process.execPath,[copiedPath],{cwd:source,env:process.env,stdio:'inherit'});
const outcome = await new Promise(resolve=>{
  child.once('error',error=>resolve({code:null,signal:null,launchError:String(error)}));
  child.once('exit',(code,signal)=>resolve({code,signal}));
});
clearInterval(heartbeat);
const resultDirectory = path.join(source,'artifacts/browser-wasm');
let result;
let readError;
try {
  const filename = path.join(resultDirectory,engine+'.json');
  assert((await fs.stat(filename)).size <= 2*1024*1024,'Unexpected result JSON size');
  result = JSON.parse(await fs.readFile(filename,'utf8'));
} catch (error) { readError = String(error); }
const files = {};
let consoleText = '';
for (const suffix of ['-console.txt','-stream.jsonl','.json']) {
  const filename = path.join(resultDirectory,engine+suffix);
  try {
    const limit = suffix==='-stream.jsonl' ? 128*1024*1024 : suffix==='-console.txt' ? 16*1024*1024 : 2*1024*1024;
    assert((await fs.stat(filename)).size <= limit,'Unexpected evidence size: '+suffix);
    const bytes = await fs.readFile(filename);
    files[engine+suffix] = {bytes:bytes.length,sha256:digest(bytes)};
    if (suffix==='-console.txt') consoleText = bytes.toString('utf8');
  } catch (error) {
    files[engine+suffix] = {error:String(error)};
  }
}
// Capture CPU evidence even when the production checker exits nonzero. A failed
// authorization request must not hide a successfully observed topology control.
// Match the production serial reader's CR normalization before exact markers.
const consoleLines = consoleText.replaceAll('\r','').split('\n');
const onlineCPUMarkers = consoleLines.flatMap(line=>{
  const match = /^\[browser-guest\] confirmed online CPUs: (.+)$/.exec(line);
  return match ? [match[1]] : [];
});
const serviceCPUMarkers = consoleLines.flatMap(line=>{
  const match = /^\[browser-guest\] confirmed service CPU availability: (.+)$/.exec(line);
  return match ? [match[1]] : [];
});
const observedTopology = {
  requestedVirtualCPUs:cpus,expectedTopology,expectedCPUList,
  observedVirtualCPUs:result?.guest?.vm?.cpus ?? null,
  observedTopology:result?.guest?.vm?.cpuTopology ?? null,
  reportedHostLogicalCPUs:result?.guest?.vm?.reportedLogicalCPUs ?? null,
  onlineCPUMarkers,serviceCPUMarkers,
  launcherMatches:result?.guest?.vm?.cpus===cpus && result?.guest?.vm?.cpuTopology===expectedTopology,
  linuxOnlineMatches:onlineCPUMarkers.length>0 && onlineCPUMarkers.every(value=>value===expectedCPUList),
  serviceAvailabilityMatches:serviceCPUMarkers.length>0 && serviceCPUMarkers.every(value=>value===expectedCPUList),
  logCaptureComplete:result?.logCapture?.complete ?? null,
  scope:'Observed topology only; neither successful authorization nor an isolated MTTCG root cause.'
};
observedTopology.matches = observedTopology.launcherMatches && observedTopology.linuxOnlineMatches && observedTopology.serviceAvailabilityMatches;
await fs.writeFile(path.join(evidenceDirectory,engine+'-topology-observation.json'),JSON.stringify(observedTopology,null,2)+'\n');
let validationError;
try {
  assert.equal(outcome.launchError,undefined);
  assert.equal(outcome.signal,null);
  assert.equal(outcome.code,0,'Production checker failed');
  assert.equal(readError,undefined);
  assert.equal(result.engine,engine);
  if (chromeSmoke) assert.equal(result.browser,chromeSmoke.actualVersion,'Browser changed after the sandbox smoke');
  else assert.match(result.browser,/^159\./,'Expected the locked Firefox 159 engine');
  assert.equal(result.passed,true);
  assert.deepEqual(result.pageErrors,[]);
  assert.equal(result.logCapture?.complete,true);
  assert.equal(result.guest?.error,undefined);
  assert.equal(result.guest?.booted,true);
  assert.equal(result.guest?.ready,true);
  assert.equal(result.guest?.accepted,true);
  assert.equal(result.guest?.checks,2);
  assert.equal(result.guest.vm.cpus,cpus);
  assert.equal(result.guest.vm.cpuTopology,expectedTopology);
  assert.equal(observedTopology.matches,true,'Actual launcher/Linux/service topology differs or was not observed');
  assert.deepEqual(result.guest.startupManifest.emulator_build,candidateBuild,'Observed guest used a different emulator build');
  assert.deepEqual(result.guest.vm.entropyConfiguration,
    materialization.configurationExperiment.entropyConfiguration,
    'The observed launcher must record the exact configured RNG backend');
  assert.deepEqual(result.guest.startupManifest.openfga_build,materialization.openfgaBuild);
  assert.deepEqual(result.guest.startupManifest.bootstrap_build,materialization.bootstrapBuild);
  assert.deepEqual(result.guest.startupManifest.files['runtime/openfga'],materialization.openfgaBinary);
  assert.deepEqual(result.guest.startupManifest.files['runtime/bootstrap'],materialization.nativeVerification.bootstrapBinary);
  assert.deepEqual(result.guest.startupManifest.files['guest.sh'],topology.selectedGuest);
  assert.deepEqual(result.guest.startupManifest.files['entrypoint.sh'],materialization.sourceFiles['browser-single/entrypoint.sh']);
  assert.deepEqual(result.passes,result.guest.passes);
  assert.equal(result.passes.length,2);
  for (let i=0;i<2;i++) {
    assert.equal(result.passes[i].id,i+1);
    assert.equal(result.passes[i].accepted,true);
    assert.deepEqual(result.passes[i].statuses,[403,204,200,204,403]);
  }
  assert(Object.values(files).every(file=>!file.error),'Required full evidence file is missing');
} catch (error) { validationError = String(error); }
const observations = consoleLines.filter(line=>
  /LAB\||\[browser-(guest|single|openfga)\]|Listening on|database system is ready|TimedOut|Elapsed\(\(\)\)|context canceled|error_kind|Error:|FATAL|PANIC/.test(line));
const excerpt = observations.slice(-40).map(line=>line.length>1500?line.slice(0,1500)+' [truncated; see full console]':line);
const summary = {
  schemaVersion:1,experiment:'guest_vcpu_topology_with_retained_idle_gc_emulator',diagnosticOnly:true,
  sourceCommit,candidateSource:overlay.sourceIdentity.candidateSource,candidateBuild,emulatorOverlay:overlay,chromeSmoke,
  workflowCommit:process.env.GITHUB_SHA ?? null,engine,requestedVirtualCPUs:cpus,topologyOverlay:topology,observedTopology,
  maxVmStarts:1,startedAt,finishedAt:new Date().toISOString(),outcome,
  browser:result?.browser ?? null,
  twoRealSequencesPassed:validationError===undefined,
  passes:result?.passes ?? null,guest:result?.guest ? {
    booted:result.guest.booted,ready:result.guest.ready,accepted:result.guest.accepted,
    checks:result.guest.checks,error:result.guest.error ?? null,vm:result.guest.vm
  }:null,
  pageErrors:result?.pageErrors ?? null,logCapture:result?.logCapture ?? null,
  failure:result?.failure ?? null,readError:readError ?? null,validationError:validationError ?? null,
  files,sourceTransformation:transformProof,jobBudget,
  openfgaBuild:materialization.openfgaBuild,openfgaBinary:materialization.openfgaBinary,
  nativeVerification:materialization.nativeVerification,bootstrapBuild:materialization.bootstrapBuild,
  configurationExperiment:materialization.configurationExperiment,
  lightweightVerification:materialization.lightweightVerification,
  kernelVerification:materialization.kernelVerification,
  entropyConfigurationScope:'The launcher records configuration; this field alone does not prove entropy delivery.',
  data:materialization.data,loader:materialization.loader,kernel:materialization.kernel,
  rootfsUnchanged:materialization.rootfsUnchanged,
  excerptScope:'Raw matching console lines may include diagnostic-tail replays; not fresh RPC results or an independent pass count.',
  matchedObservationLines:files[engine+'-console.txt']?.error ? null : observations.length,excerpt
};
await fs.writeFile(path.join(evidenceDirectory,engine+'-summary.json'),JSON.stringify(summary,null,2)+'\n');
console.log('VCPU_BROWSER_GATE '+JSON.stringify(summary));
if (validationError!==undefined) process.exitCode=1;
