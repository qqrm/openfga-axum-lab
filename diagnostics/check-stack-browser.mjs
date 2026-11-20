// Run the original application checker in one fresh Chromium VM per datastore arm.
// A component or one-browser result must never claim the complete production gate.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {constants, promises as fs} from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawn,spawnSync} from 'node:child_process';
import {record,verifyBuild} from './candidate-emulator.mjs';
import {parseStrictJson} from './memory-component-protocol.mjs';
import {transformStackRuntime} from './stack-runtime-transform.mjs';

const sourceCommit = '444b069daf16e9606a0bbf1ea0aea7f4d61b97d8';
const emulatorSource = '6f35f24b7ca345cf47c873becee6a41ef94e65a9';
const engine = 'chromium';
const cpus = 2;
const expectedTopology = '2,sockets=2,cores=1,threads=1';
const expectedCPUList = '0-1';
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const readJSON = async filename => JSON.parse(await fs.readFile(filename,'utf8'));
const mutablePaths = ['guest.sh','entrypoint.sh','runtime/startup-manifest.json','runtime/derived-runtime.json'];

// Reconstruct the original source, independently of the checker's complete flag.
// Only original whole QEMU telemetry chunks are excluded from marker parsing;
// their bytes and sequence positions remain in the archived console and JSONL.
export function verifySourceCapture(streamBytes,consoleBytes,metadata) {
  assert(Buffer.isBuffer(streamBytes) && streamBytes.length<=128*1024*1024);
  assert(Buffer.isBuffer(consoleBytes) && consoleBytes.length<=16*1024*1024);
  const stream=streamBytes.toString('utf8');
  assert.deepEqual(Buffer.from(stream,'utf8'),streamBytes,'Invalid UTF-8 stream evidence');
  assert(stream.length===0||stream.endsWith('\n'),'Truncated final source batch');
  const rows=stream.length ? stream.slice(0,-1).split('\n') : [];
  let chunks=0,bytes=0,telemetryChunks=0,telemetryBytes=0;
  const raw=[],uart=[];
  for (const line of rows) {
    assert(line.length>0,'Empty source batch');
    const row=parseStrictJson(line);
    assert.deepEqual(Object.keys(row).sort(),['firstSequence','receivedAt','texts']);
    assert.equal(row.firstSequence,chunks+1,'Source sequence gap');
    assert.equal(typeof row.receivedAt,'string');
    assert(Number.isFinite(Date.parse(row.receivedAt)),'Invalid batch receipt timestamp');
    assert(Array.isArray(row.texts)&&row.texts.length>0&&row.texts.length<=1024,'Invalid source batch size');
    for (const text of row.texts) {
      assert.equal(typeof text,'string');
      raw.push(text);chunks++;bytes+=Buffer.byteLength(text);
      assert(chunks<=1000000&&bytes<=16*1024*1024,'Source capture exceeded production bounds');
      if (text.startsWith('[browser-qemu] ')) {
        const match=/^\[browser-qemu\] (\{[^\r\n]*\})\n$/.exec(text);
        assert(match&&match[0]===text,'Malformed or mixed QEMU telemetry chunk');
        const event=parseStrictJson(match[1]);
        assert.equal(typeof event.event,'string');
        telemetryChunks++;telemetryBytes+=Buffer.byteLength(text);
      } else uart.push(text);
    }
  }
  assert.deepEqual(Buffer.from(raw.join(''),'utf8'),consoleBytes,'Source JSONL does not reproduce the exact console');
  assert.equal(metadata?.format,'browser-lab-log-batches-v1');
  assert.equal(metadata?.complete,true,'Incomplete production source capture');
  assert.equal(metadata?.chunks,chunks);assert.equal(metadata?.expectedSequence,chunks);
  assert.equal(metadata?.batches,rows.length);assert.equal(metadata?.bytes,bytes);
  assert.equal(metadata?.error,undefined);
  const text=uart.join('').replaceAll('\r','');
  const lines=text.split('\n');
  const pending=lines.at(-1);
  return {lines,report:{verified:true,chunks,batches:rows.length,bytes,telemetryChunks,telemetryBytes,
    consoleBytes:consoleBytes.length,streamBytes:streamBytes.length,
    partialProtocolRecord:pending.includes('LAB|')||(pending.length>0&&'LAB|'.startsWith(pending))}};
}

// A normal guest failure or a real VM resource failure is a useful paired
// observation. Missing proof, broken instrumentation and incomplete capture are not.
export function classifyContinuation({checkerLaunched,checkerExited,outcome,result,captureVerified,
  topologyMatches,resultIdentityMatches,applicationAcceptancePassed}) {
  const deny=reason=>({permitted:false,reason});
  if (!checkerLaunched||!checkerExited||outcome?.launchError||outcome?.signal!==null||![0,1].includes(outcome?.code))
    return deny('The original checker did not finish normally');
  if (!resultIdentityMatches||!captureVerified||!topologyMatches)
    return deny('Result identity, complete raw capture or actual guest topology was not verified');
  if (applicationAcceptancePassed) return {permitted:true,reason:'Two original application acceptance executions passed'};
  if (outcome.code!==1||result?.passed!==false) return deny('Unexpected checker outcome or wrapper validation failure');
  if (result?.guest?.booted!==true) return deny('No observed Linux guest boot');
  const guestError=result.guest.error;
  const pageErrors=result.pageErrors;
  if (!Array.isArray(pageErrors)) return deny('Missing browser error observations');
  if (pageErrors.length===0&&typeof guestError==='string'&&/^LAB\|(boot|verification|service)-failed(?:\||$)/.test(guestError))
    return {permitted:true,reason:'The real full-stack guest reported a functional startup or acceptance failure'};
  if (pageErrors.length===0&&guestError===undefined&&/^TimeoutError: page\.waitForFunction: Timeout (1200000|180000)ms exceeded\./.test(result.failure??''))
    return {permitted:true,reason:'The original checker exhausted its unchanged guest acceptance wait'};
  const errors=[...pageErrors,...(typeof guestError==='string'?[guestError]:[])];
  const vmFailure=text=>typeof text==='string'&&/out of memory|out-of-memory|\bOOM\b|WebAssembly aborted|Guest emulator exited with status|RuntimeError: (?:memory access out of bounds|unreachable)/i.test(text);
  if (errors.length>0&&errors.every(vmFailure))
    return {permitted:true,reason:'The real VM reported a runtime/resource failure with complete retained capture'};
  return deny('Failure was not identified as a functional guest or VM result');
}

async function main() {
  const datastoreEngine=process.env.BROWSER_STACK_DATASTORE;
  assert(['memory','postgres'].includes(datastoreEngine),'Expected one explicit OpenFGA datastore arm');
  const source=path.resolve('source-candidate');
  const wasmRoot=path.join(source,'browser-output/wasm');
  const evidenceDirectory=path.resolve('artifacts/pr36-stack-continuation',datastoreEngine);
  await fs.mkdir(evidenceDirectory,{recursive:true});
  const summaryPath=path.join(evidenceDirectory,'summary.json');
  const summary={schemaVersion:1,experiment:'sequential_full_stack_datastore_comparison',diagnosticOnly:true,
    engine,datastoreEngine,sourceCommit,emulatorSource,workflowCommit:process.env.GITHUB_SHA??null,
    runId:Number(process.env.GITHUB_RUN_ID),runAttempt:Number(process.env.GITHUB_RUN_ATTEMPT),
    requestedVirtualCPUs:cpus,maxVmStarts:1,checkerLaunched:false,checkerExited:false,
    applicationAcceptancePassed:false,productionGatePassed:false,productionAcceptance:false,
    comparisonContinuationPermitted:false,comparisonContinuationReason:'Preflight has not completed',
    logCaptureComplete:false,phase:'preflight',files:{},startedAt:null,finishedAt:null};
  let phase='preflight';
  try {
    assert.equal(process.env.BROWSER_DIAGNOSTIC_ENGINE,engine);
    assert.equal(process.env.BROWSER_DIAGNOSTIC_VCPUS,'2');
    assert.equal(process.env.GITHUB_RUN_ATTEMPT,'1');
    assert.equal(process.env.EMULATOR_SOURCE_SHA,emulatorSource);
    const materializationPath='artifacts/openfga10-bfs-browser-gate/materialization.json';
    const materialization=await readJSON(materializationPath);
    const emulatorOverlayPath='artifacts/pr36-idle-gc-gate/emulator-overlay.json';
    const overlay=await readJSON(emulatorOverlayPath);
    const stackOverlayPath=path.join(evidenceDirectory,'stack-overlay.json');
    const stack=await readJSON(stackOverlayPath);
    assert.equal(overlay.kind,'verified_emulator_only_overlay');
    assert.equal(overlay.sourceIdentity.candidateSource,emulatorSource);
    assert.equal(overlay.all17NativeInputsUnchanged,true);
    assert.equal(overlay.authorizationDeadlinesChanged,false);
    assert.equal(overlay.browserAcceptanceVerified,false);
    assert.deepEqual(overlay.baselineMaterialization,await record(materializationPath));
    assert.equal(stack.kind,'verified_full_stack_datastore_overlay');
    assert.equal(stack.baselineSource,sourceCommit);
    assert.equal(stack.datastoreEngine,datastoreEngine);
    assert.equal(stack.emulatorSource,emulatorSource);
    assert.deepEqual(stack.baselineMaterialization,await record(materializationPath));
    assert.deepEqual(stack.emulatorOverlay,await record(emulatorOverlayPath));
    assert.deepEqual(stack.topologyOverlay,await record('artifacts/pr36-vcpu-gate/topology-overlay.json'));
    assert.deepEqual(stack.transformImplementation,await record('diagnostics/stack-runtime-transform.mjs'));
    assert.deepEqual(stack.beforeFiles,{'guest.sh':overlay.preservedFiles['guest.sh'],
      'entrypoint.sh':overlay.preservedFiles['entrypoint.sh'],
      'runtime/startup-manifest.json':overlay.startupManifest,'runtime/derived-runtime.json':overlay.derivedRuntime});
    const transformedRuntime=transformStackRuntime({
      guest:await fs.readFile(path.join(source,'browser-single/pages/guest.sh'),'utf8'),
      entrypoint:await fs.readFile(path.join(source,'browser-single/entrypoint.sh'),'utf8'),datastore:datastoreEngine});
    assert.deepEqual(stack.transform,transformedRuntime.proof);
    assert.equal(await fs.readFile(path.join(wasmRoot,'guest.sh'),'utf8'),transformedRuntime.guest);
    assert.equal(await fs.readFile(path.join(wasmRoot,'entrypoint.sh'),'utf8'),transformedRuntime.entrypoint);
    for (const key of ['all17NativeInputsUnchanged','applicationBinariesUnchanged','rootfsUnchanged',
      'originalCheckerUnchanged','sourceCheckoutsUnchanged']) assert.equal(stack[key],true,'Missing stack invariant: '+key);
    assert.equal(stack.authorizationDeadlinesChanged,false);
    assert.equal(stack.productionAcceptance,false);
    assert.deepEqual(stack.nativeInputs,overlay.nativeInputs);
    assert.equal(Object.keys(stack.nativeInputs).length,17);
    assert.deepEqual(Object.keys(stack.afterFiles).sort(),[...mutablePaths].sort());
    assert.deepEqual(stack.startupManifest,stack.afterFiles['runtime/startup-manifest.json']);
    assert.deepEqual(stack.derivedRuntime,stack.afterFiles['runtime/derived-runtime.json']);
    assert.equal(materialization.expectedRuntimeConfiguration.virtualCPUs,2);
    assert.equal(materialization.expectedRuntimeConfiguration.memoryMiB,512);
    assert.equal(materialization.expectedRuntimeConfiguration.accel,'tcg,tb-size=500,thread=multi');
    for (const [name,expected] of Object.entries(stack.nativeInputs)) {
      assert.deepEqual(await record(path.join(source,name)),{bytes:expected.bytes,sha256:expected.sha256},'Native input changed: '+name);
    }
    const candidateBuild=await verifyBuild(path.resolve('emulator-source'),path.resolve('candidate-emulator'));
    assert.deepEqual(candidateBuild,overlay.candidateEmulator);
    for (const [name,expected] of Object.entries(overlay.preservedFiles)) {
      if (!mutablePaths.includes(name)) assert.deepEqual(await record(path.join(wasmRoot,name)),expected,'Baseline served input differs: '+name);
    }
    for (const [name,expected] of Object.entries(candidateBuild.files)) {
      assert.deepEqual(await record(path.join(wasmRoot,'runtime',name)),expected);
    }
    async function servedFiles(directory,prefix='') {
      const files={};
      for (const item of await fs.readdir(directory,{withFileTypes:true})) {
        const name=prefix+item.name,filename=path.join(directory,item.name);
        assert(!item.isSymbolicLink(),'Unexpected served symlink');
        if(item.isDirectory())Object.assign(files,await servedFiles(filename,name+'/'));
        else {assert(item.isFile());files[name]=await record(filename);}
      }
      return files;
    }
    assert.deepEqual(await servedFiles(wasmRoot),{...stack.preservedFiles,...stack.afterFiles},'Served files differ from the complete stack overlay proof');
    assert.deepEqual(await fs.readFile(path.join(wasmRoot,'launcher.js')),await fs.readFile(path.join(source,'browser-single/pages/launcher.js')));
    const startup=await readJSON(path.join(wasmRoot,'runtime/startup-manifest.json'));
    const derived=await readJSON(path.join(wasmRoot,'runtime/derived-runtime.json'));
    assert.deepEqual(startup.files['guest.sh'],stack.afterFiles['guest.sh']);
    assert.deepEqual(startup.files['entrypoint.sh'],stack.afterFiles['entrypoint.sh']);
    assert.deepEqual(startup.emulator_build,candidateBuild);
    assert.equal(derived.startup_overlay.sha256,stack.startupManifest.sha256);
    assert.equal(derived.rootfs_changed,false);
    const chromeSmoke=await readJSON('artifacts/pr36-vcpu-gate/chromium/system-chrome-smoke.json');
    assert.equal(chromeSmoke.passed,true);
    assert.equal(chromeSmoke.launchOptions.chromiumSandbox,true);
    assert.equal(chromeSmoke.launchOptions.channel,'chrome');
    assert.deepEqual(await record(chromeSmoke.executable.path),{bytes:chromeSmoke.executable.bytes,sha256:chromeSmoke.executable.sha256});
    summary.chromeSmoke=chromeSmoke;
    summary.stackOverlay=await record(stackOverlayPath);
    summary.candidateBuild=candidateBuild;
    summary.openfgaBuild=materialization.openfgaBuild;
    summary.openfgaBinary=materialization.openfgaBinary;
    summary.nativeVerification=materialization.nativeVerification;
    summary.bootstrapBuild=materialization.bootstrapBuild;
    summary.configurationExperiment=materialization.configurationExperiment;
    summary.lightweightVerification=materialization.lightweightVerification;
    summary.kernelVerification=materialization.kernelVerification;
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
    const clean=spawnSync('git',['diff','--exit-code','HEAD','--'],{cwd:source,encoding:'utf8'});
    assert.equal(clean.status,0,'The pinned source checkout changed');
    const originalPath=path.join(source,'browser-single/check-pages.mjs');
    const original=await fs.readFile(originalPath,'utf8');
    const initializer="for (const [name, engine] of [['chromium',chromium],['firefox',firefox]]) {";
    const singleton="for (const [name, engine] of [['chromium',chromium]]) {";
    const originalLaunch='browser = await engine.launch({headless:true});';
    const candidateLaunch="browser = await engine.launch({headless:true,channel:'chrome',chromiumSandbox:true,args:['--enable-automation']});";
    assert.equal(original.split(initializer).length,2,'Production checker loop changed');
    assert.equal(original.split(originalLaunch).length,2,'Production browser launch changed');
    const transformed=original.replace(initializer,()=>singleton).replace(originalLaunch,()=>candidateLaunch);
    assert.equal(transformed.split(singleton).length,2);
    assert.equal(transformed.split(candidateLaunch).length,2);
    assert.equal(transformed.replace(singleton,()=>initializer).replace(candidateLaunch,()=>originalLaunch),original,
      'Only the engine loop and explicit Chromium channel/sandbox may change');
    assert.deepEqual({bytes:Buffer.byteLength(original),sha256:digest(original)},materialization.sourceFiles['browser-single/check-pages.mjs']);
    const copiedPath=path.join(source,'browser-single/check-pages.singleton-stack-'+datastoreEngine+'.mjs');
    await fs.writeFile(copiedPath,transformed,{flag:'wx'});
    const syntax=spawnSync(process.execPath,['--check',copiedPath],{cwd:source,stdio:'inherit'});
    assert.equal(syntax.status,0,'Singleton production checker syntax failed');
    const transformProof={sourceCommit,engine,datastoreEngine,
      original:{bytes:Buffer.byteLength(original),sha256:digest(original)},
      transformed:{bytes:Buffer.byteLength(transformed),sha256:digest(transformed)},
      copiedRelativePath:path.relative(source,copiedPath),inverseByteComparison:true,
      onlyChanges:['literal engine loop initializer','explicit system Chrome channel with sandbox'],
      maxVmStarts:1,firstWaitMilliseconds:1200000,secondWaitMilliseconds:180000,diagnosticOnly:true};
    await fs.writeFile(path.join(evidenceDirectory,'checker-transform.json'),JSON.stringify(transformProof,null,2)+'\n',{flag:'wx'});
    summary.sourceTransformation=transformProof;
    const jobTiming=await readJSON('prepared/job-timing.json');
    assert.equal(jobTiming.source,'GitHub Actions current job API');
    assert.equal(jobTiming.name,'Browser full-stack datastore comparison');
    assert.equal(jobTiming.run_id,Number(process.env.GITHUB_RUN_ID));
    assert.equal(jobTiming.run_attempt,1);
    assert.equal(jobTiming.head_sha,process.env.GITHUB_SHA);
    assert(Number.isSafeInteger(jobTiming.id)&&jobTiming.id>0);
    summary.job=jobTiming;
    if(datastoreEngine==='postgres') {
      const previousDirectory=path.resolve('artifacts/pr36-stack-continuation/memory');
      const previousPath=path.join(previousDirectory,'summary.json');
      const previous=await readJSON(previousPath);
      for(const [key,value] of Object.entries({sourceCommit,emulatorSource,engine,datastoreEngine:'memory',
        workflowCommit:process.env.GITHUB_SHA,runId:Number(process.env.GITHUB_RUN_ID),runAttempt:1,
        checkerLaunched:true,checkerExited:true,comparisonContinuationPermitted:true,logCaptureComplete:true})) assert.equal(previous[key],value,'Previous arm identity/continuation differs: '+key);
      assert.deepEqual(previous.job,jobTiming,'The paired arms must use the same actual Actions job');
      assert.deepEqual(previous.chromeSmoke.executable,chromeSmoke.executable,'The paired arms must use the same Chrome binary');
      assert(Number.isFinite(Date.parse(previous.finishedAt))&&Date.parse(previous.finishedAt)<=Date.now());
      for(const name of ['chromium-console.txt','chromium-stream.jsonl','chromium.json']) {
        assert.deepEqual(await record(path.join(previousDirectory,name)),previous.files[name],'Previous arm evidence changed: '+name);
      }
      summary.precedingArm={summary:await record(previousPath),applicationAcceptancePassed:previous.applicationAcceptancePassed,
        comparisonContinuationReason:previous.comparisonContinuationReason,finishedAt:previous.finishedAt};
    }
    const checkedAt=Date.now(),elapsed=checkedAt-Date.parse(jobTiming.started_at);
    const latestLaunch=datastoreEngine==='memory'?8*60000:(65-28)*60000;
    const jobBudget={...jobTiming,checkedAt:new Date(checkedAt).toISOString(),
      jobElapsedMilliseconds:Number.isFinite(elapsed)?elapsed:null,outerJobMinutes:65,runtimeStepMinutes:25,
      minimumRemainingJobMilliseconds:datastoreEngine==='postgres'?28*60000:null,
      latestLaunchJobElapsedMilliseconds:latestLaunch,
      launchPermitted:Number.isFinite(elapsed)&&elapsed>=0&&elapsed<=latestLaunch};
    await fs.writeFile(path.join(evidenceDirectory,'job-budget.json'),JSON.stringify(jobBudget,null,2)+'\n',{flag:'wx'});
    summary.jobBudget=jobBudget;
    assert(jobBudget.launchPermitted,'Insufficient or unverified remaining Actions job budget');
    const resultDirectory=path.join(source,'artifacts/browser-wasm');
    try {assert.equal((await fs.readdir(resultDirectory)).length,0,'Previous checker outputs must be archived before the next arm');}
    catch(error){if(error.code!=='ENOENT')throw error;}
    phase='checker';summary.phase=phase;summary.startedAt=new Date().toISOString();
    console.log('STACK_BROWSER_GATE '+JSON.stringify({phase:'checker_launch',datastoreEngine,...transformProof,jobBudget}));
    const heartbeat=setInterval(()=>console.log('STACK_BROWSER_GATE '+JSON.stringify({phase:'acceptance_running',datastoreEngine,
      elapsedMs:Date.now()-Date.parse(summary.startedAt),engine,requestedVirtualCPUs:cpus})),60000);
    const child=spawn(process.execPath,[copiedPath],{cwd:source,env:process.env,stdio:'inherit'});
    child.once('spawn',()=>{summary.checkerLaunched=true;});
    const outcome=await new Promise(resolve=>{
      child.once('error',error=>resolve({code:null,signal:null,launchError:String(error)}));
      child.once('exit',(code,signal)=>{summary.checkerExited=true;resolve({code,signal});});
    });
    clearInterval(heartbeat);summary.outcome=outcome;
    phase='retained_evidence';summary.phase=phase;
    const required=['chromium-console.txt','chromium-stream.jsonl','chromium.json'];
    const allowed=[...required,'chromium-console-tail.txt','chromium.png'];
    const resultNames=await fs.readdir(resultDirectory).catch(error=>{if(error.code==='ENOENT')return [];throw error;});
    for(const name of resultNames) {
      assert(allowed.includes(name),'Unexpected original checker output: '+name);
      const filename=path.join(resultDirectory,name),destination=path.join(evidenceDirectory,name);
      const limit=name.endsWith('-stream.jsonl')?128*1024*1024:name.endsWith('-console.txt')?16*1024*1024:name.endsWith('.png')?16*1024*1024:2*1024*1024;
      const stat=await fs.lstat(filename);
      assert(stat.isFile()&&!stat.isSymbolicLink()&&stat.size<=limit,'Unexpected evidence type/size: '+name);
      await fs.copyFile(filename,destination,constants.COPYFILE_EXCL);
      const originalFile=await record(filename);
      assert.deepEqual(await record(destination),originalFile,'Archived output bytes differ: '+name);
      summary.files[name]=originalFile;
    }
    for(const name of required)assert(summary.files[name],'Required original checker evidence is missing: '+name);
    const result=await readJSON(path.join(evidenceDirectory,'chromium.json'));
    summary.result=result;
    summary.browser=result.browser??null;
    const captured=verifySourceCapture(await fs.readFile(path.join(evidenceDirectory,'chromium-stream.jsonl')),
      await fs.readFile(path.join(evidenceDirectory,'chromium-console.txt')),result.logCapture);
    summary.sourceCapture=captured.report;summary.logCaptureComplete=true;
    const consoleLines=captured.lines;
    const onlineCPUMarkers=consoleLines.flatMap(line=>{const match=/^\[browser-guest\] confirmed online CPUs: (.+)$/.exec(line);return match?[match[1]]:[];});
    const serviceCPUMarkers=consoleLines.flatMap(line=>{const match=/^\[browser-guest\] confirmed service CPU availability: (.+)$/.exec(line);return match?[match[1]]:[];});
    const observedTopology={requestedVirtualCPUs:cpus,expectedTopology,expectedCPUList,
      observedVirtualCPUs:result?.guest?.vm?.cpus??null,observedTopology:result?.guest?.vm?.cpuTopology??null,
      reportedHostLogicalCPUs:result?.guest?.vm?.reportedLogicalCPUs??null,onlineCPUMarkers,serviceCPUMarkers,
      launcherMatches:result?.guest?.vm?.cpus===cpus&&result?.guest?.vm?.cpuTopology===expectedTopology,
      linuxOnlineMatches:onlineCPUMarkers.length>0&&onlineCPUMarkers.every(value=>value===expectedCPUList),
      serviceAvailabilityMatches:serviceCPUMarkers.length>0&&serviceCPUMarkers.every(value=>value===expectedCPUList)};
    observedTopology.matches=observedTopology.launcherMatches&&observedTopology.linuxOnlineMatches&&observedTopology.serviceAvailabilityMatches;
    summary.observedTopology=observedTopology;
    await fs.writeFile(path.join(evidenceDirectory,'topology-observation.json'),JSON.stringify(observedTopology,null,2)+'\n',{flag:'wx'});
    let observedStartupMatches=false;
    try {assert.deepEqual(result.guest?.startupManifest,startup);observedStartupMatches=true;} catch {}
    const resultIdentityMatches=result.engine===engine&&result.browser===chromeSmoke.actualVersion&&observedStartupMatches;
    summary.resultIdentityMatches=resultIdentityMatches;
    summary.stackObservations=consoleLines.filter(line=>line.startsWith('STACK|')).map(line=>{
      const match=/^STACK\|(phase|entrypoint-exit|guest-failure)\|(\{.*\})$/.exec(line);
      assert(match,'Malformed stack observation');
      const value=parseStrictJson(match[2]);
      assert(Number.isSafeInteger(value.guest_seconds)&&value.guest_seconds>=0);
      assert(typeof value.uptime_seconds==='string'&&/^(?:[0-9]+(?:\.[0-9]+)?|unavailable)$/.test(value.uptime_seconds));
      if(match[1]==='guest-failure') {
        assert.equal(value.clock_scope,'guest-wrapper');
        assert(['entrypoint-exited','readiness-deadline'].includes(value.reason));
        assert.equal(value.readiness_deadline_seconds,900);
      } else {
        assert.equal(value.clock_scope,'entrypoint');assert.equal(value.datastore,datastoreEngine);
        assert.equal(typeof value.phase,'string');
      }
      return {kind:match[1],...value};
    });
    let validationError;
    try {
      assert.equal(outcome.launchError,undefined);assert.equal(outcome.signal,null);assert.equal(outcome.code,0,'Production checker failed');
      assert(resultIdentityMatches,'Browser result identity differs');
      assert.equal(result.passed,true);assert.deepEqual(result.pageErrors,[]);assert.equal(result.logCapture?.complete,true);
      assert.equal(captured.report.partialProtocolRecord,false,'Truncated application protocol record');
      assert.equal(result.guest?.error,undefined);assert.equal(result.guest?.booted,true);assert.equal(result.guest?.ready,true);
      assert.equal(result.guest?.accepted,true);assert.equal(result.guest?.checks,2);
      assert.equal(result.guest.vm.cpus,cpus);assert.equal(result.guest.vm.cpuTopology,expectedTopology);
      assert.equal(observedTopology.matches,true,'Actual launcher/Linux/service topology differs');
      assert.deepEqual(result.guest.startupManifest,startup,'Observed guest startup manifest differs from the exact served overlay');
      assert.deepEqual(result.guest.startupManifest.emulator_build,candidateBuild);
      assert.deepEqual(result.guest.vm.entropyConfiguration,materialization.configurationExperiment.entropyConfiguration);
      assert.deepEqual(result.guest.startupManifest.openfga_build,materialization.openfgaBuild);
      assert.deepEqual(result.guest.startupManifest.bootstrap_build,materialization.bootstrapBuild);
      assert.deepEqual(result.guest.startupManifest.files['runtime/openfga'],materialization.openfgaBinary);
      assert.deepEqual(result.guest.startupManifest.files['runtime/bootstrap'],materialization.nativeVerification.bootstrapBinary);
      assert.deepEqual(result.guest.startupManifest.files['guest.sh'],stack.afterFiles['guest.sh']);
      assert.deepEqual(result.guest.startupManifest.files['entrypoint.sh'],stack.afterFiles['entrypoint.sh']);
      assert.deepEqual(result.passes,result.guest.passes);assert.equal(result.passes.length,2);
      for(let i=0;i<2;i++) {
        assert.equal(result.passes[i].id,i+1);assert.equal(result.passes[i].accepted,true);
        assert.deepEqual(result.passes[i].statuses,[403,204,200,204,403]);
      }
      const markers=consoleLines.filter(line=>line.startsWith('LAB|'));
      assert.deepEqual(markers.map(line=>line.startsWith('LAB|result|')?'LAB|result|'+line.split('|')[2]:line),
        ['LAB|boot','LAB|ready','LAB|verification-start|1','LAB|result|1','LAB|verification-start|2','LAB|result|2'],
        'Original indexed application protocol markers differ');
      for(let i=0;i<2;i++) {
        const line=markers[3+i*2],prefix='LAB|result|'+(i+1)+'|';
        assert(line.startsWith(prefix));
        assert.deepEqual({...parseStrictJson(line.slice(prefix.length)),id:i+1},result.passes[i]);
      }
    } catch(error){validationError=String(error);}
    summary.applicationAcceptancePassed=validationError===undefined;
    summary.validationError=validationError??null;
    summary.passes=result.passes??null;
    summary.pageErrors=result.pageErrors??null;
    summary.logCapture=result.logCapture??null;
    summary.failure=result.failure??null;
    const continuation=classifyContinuation({checkerLaunched:summary.checkerLaunched,checkerExited:summary.checkerExited,
      outcome,result,captureVerified:!captured.report.partialProtocolRecord,topologyMatches:observedTopology.matches,
      resultIdentityMatches,applicationAcceptancePassed:summary.applicationAcceptancePassed});
    summary.comparisonContinuationPermitted=continuation.permitted;
    summary.comparisonContinuationReason=continuation.reason;
    summary.observedStages=consoleLines.filter(line=>/STACK|\[browser-(guest|single)\]|Listening on|database system is ready/.test(line));
    summary.lastObservedLines=consoleLines.filter(line=>/LAB\||\[browser-(guest|single|openfga)\]|Listening on|database system is ready|TimedOut|Elapsed\(\(\)\)|context canceled|error_kind|Error:|FATAL|PANIC/.test(line))
      .slice(-40).map(line=>line.length>1500?line.slice(0,1500)+' [truncated; see full console]':line);
    // Free only the exact original-output files after their byte-identical copies
    // have been retained. The next fresh VM cannot reuse the preceding result.
    for(const name of resultNames)await fs.unlink(path.join(resultDirectory,name));
    if(resultNames.length)await fs.rmdir(resultDirectory);
    phase='completed';summary.phase=phase;
  } catch(error) {
    summary.phase=phase;
    summary.applicationAcceptancePassed=false;
    summary.validationError=String(error);
    summary.comparisonContinuationPermitted=false;
    summary.comparisonContinuationReason='Setup, instrumentation, integrity or capture verification did not complete';
    console.error('STACK_BROWSER_GATE '+JSON.stringify({phase,datastoreEngine,error:String(error)}));
  } finally {
    summary.finishedAt=new Date().toISOString();
    await fs.writeFile(summaryPath,JSON.stringify(summary,null,2)+'\n',{flag:'wx'});
  }
  console.log('STACK_BROWSER_GATE '+JSON.stringify({phase:'finished',datastoreEngine,
    applicationAcceptancePassed:summary.applicationAcceptancePassed,productionGatePassed:false,
    comparisonContinuationPermitted:summary.comparisonContinuationPermitted,validationError:summary.validationError??null}));
  if(!summary.applicationAcceptancePassed)process.exitCode=1;
}

if (process.argv[1] && path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) await main();
