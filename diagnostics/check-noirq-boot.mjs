// One retained real Firefox boot observation. This is not an acceptance gate.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';
import {createReadStream,promises as fs} from 'node:fs';
import {createRequire} from 'node:module';
import {performance} from 'node:perf_hooks';
import path from 'node:path';
import {pathToFileURL} from 'node:url';

const sourceCommit='1f10fb8b17cf09ae18b0003fbf0b7e731b91b6aa';
assert.equal(process.env.BROWSER_DIAGNOSTIC_ENGINE,'firefox');
assert.equal(process.env.GITHUB_RUN_ATTEMPT,'1','No automatic or manual retry is part of this diagnostic');
const source=path.resolve('source-candidate');
const evidenceDirectory=path.resolve('artifacts/noirq-boot-diagnostic');
const root=path.join(source,'browser-output');
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
const readJSON=async file=>JSON.parse(await fs.readFile(file,'utf8'));
const materialization=await readJSON(path.join(evidenceDirectory,'materialization.json'));
assert.equal(materialization.sourceCommit,sourceCommit);
assert.equal(materialization.configurationExperiment.value,'acpi=noirq');
assert.equal(materialization.configurationExperiment.engine,'firefox');
assert.equal(materialization.productionAcceptance,false);
assert.equal(materialization.hostObservationMilliseconds,300000);
for(const [name,expected] of Object.entries(materialization.sourceFiles)){
  const bytes=await fs.readFile(path.join(source,name));
  assert.equal(bytes.length,expected.bytes);
  assert.equal(digest(bytes),expected.sha256,'Source changed after materialization: '+name);
}
const servedLauncher=await fs.readFile(path.join(root,'wasm/launcher.js'));
assert.deepEqual({bytes:servedLauncher.length,sha256:digest(servedLauncher)},
  materialization.configurationExperiment.candidateLauncher);
const startup=await readJSON(path.join(root,'wasm/runtime/startup-manifest.json'));
assert.deepEqual(startup.openfga_build,materialization.openfgaBuild);
assert.deepEqual(startup.bootstrap_build,materialization.bootstrapBuild);
assert.deepEqual(startup.files['guest.sh'],materialization.configurationExperiment.guest);

const require=createRequire(path.join(source,'e2e/package.json'));
const {firefox}=require('@playwright/test');
const logModule=path.join(source,'browser-single/pages/log-stream.mjs');
const {createGuestLogCollector,drainGuestLogWithin}=await import(pathToFileURL(logModule).href);
const jobTiming=await readJSON(path.resolve('prepared/job-timing.json'));
assert.equal(jobTiming.source,'GitHub Actions current job API');
assert.equal(jobTiming.name,'Noirq kernel boot (firefox)');
assert.equal(jobTiming.run_id,Number(process.env.GITHUB_RUN_ID));
assert.equal(jobTiming.run_attempt,1);
assert.equal(jobTiming.head_sha,process.env.GITHUB_SHA);
const elapsed=Date.now()-Date.parse(jobTiming.started_at);
const jobBudget={...jobTiming,checkedAt:new Date().toISOString(),
  jobElapsedMilliseconds:Number.isFinite(elapsed)?elapsed:null,
  latestLaunchJobElapsedMilliseconds:720000,outerJobMinutes:20,runtimeStepMinutes:7,
  launchPermitted:Number.isFinite(elapsed)&&elapsed>=0&&elapsed<=720000};
await fs.writeFile(path.join(evidenceDirectory,'firefox-job-budget.json'),JSON.stringify(jobBudget,null,2)+'\n');
if(!jobBudget.launchPermitted){
  console.log('NOIRQ_BOOT_DIAGNOSTIC '+JSON.stringify({phase:'preflight_stop',sourceCommit,jobBudget,
    vmStartRequested:false,bootObserved:null,acceptanceVerified:false}));
  throw new Error('Refusing a VM without eight minutes of verified remaining job time');
}

const types={'.html':'text/html','.js':'text/javascript','.mjs':'text/javascript','.css':'text/css',
  '.wasm':'application/wasm','.json':'application/json','.sh':'text/plain'};
const server=createServer(async(request,response)=>{
  try{
    const url=new URL(request.url,'http://localhost');
    const file=path.resolve(root,'.'+decodeURIComponent(url.pathname)+(url.pathname.endsWith('/')?'index.html':''));
    assert(file.startsWith(root+path.sep));
    const stat=await fs.stat(file); assert(stat.isFile());
    // Exercise the unchanged production isolation service worker.
    response.writeHead(200,{'Content-Type':types[path.extname(file)]||'application/octet-stream','Content-Length':stat.size});
    const stream=createReadStream(file); stream.on('error',()=>response.destroy()); stream.pipe(response);
  }catch{response.writeHead(404);response.end();}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const url='http://127.0.0.1:'+server.address().port+'/wasm/';
const guestLog=createGuestLogCollector();
const receipts=[],markers={},optionalObservations=[];
const errors=[];
let page,browser,cutoff={},capture,actualArguments=null,failure=null,cleanupError=null;
let browserVersion=null,vmStartRequested=false,bootObserved=false,observationEnded=false;
let observationElapsedMilliseconds=null;
let scanCarry='',scanIndeterminate=false;
const startedAt=new Date().toISOString();
const startedMono=performance.now();
const hostDeadline=startedMono+300000;
const remaining=()=>Math.max(0,hostDeadline-performance.now());
let rejectFailure;
const pageFailure=new Promise((_,reject)=>{rejectFailure=reject;});
pageFailure.catch(()=>{});
const recordFailure=error=>{
  errors.push(String(error));
  rejectFailure(error instanceof Error?error:new Error(String(error)));
};
let deadlineTimer;
const hostTimeout=new Promise((_,reject)=>{
  deadlineTimer=setTimeout(()=>reject(new Error('Five-minute host boot observation deadline exceeded')),300000);
});
hostTimeout.catch(()=>{});
const wait=operation=>Promise.race([operation,pageFailure,hostTimeout]);
const patterns={
  onlineCPUs:/\[browser-guest\] confirmed online CPUs: 0-1$/,
  serviceCPUs:/\[browser-guest\] confirmed service CPU availability: 0-1$/,
  hpet:/clocksource: Switched to clocksource hpet$/,
  rootfs:/VFS: Mounted root \(iso9660 filesystem\) readonly on device 254:0\.$/
};
const observe=(batch,receipt)=>{
  const text=scanCarry+batch.map(chunk=>chunk.text).join('');
  const lines=text.replaceAll('\r','').split('\n');
  scanCarry=lines.pop();
  if(scanCarry.length>65536){scanCarry=scanCarry.slice(-65536);scanIndeterminate=true;}
  for(const line of lines){
    for(const [name,pattern] of Object.entries(patterns)){
      if(!markers[name]&&pattern.test(line))markers[name]={...receipt,line:line.slice(-2048)};
    }
    if(optionalObservations.length<24&&
      /Intel MultiProcessor Specification|PCI->APIC IRQ transform|ACPI.*(Error|HPET|PM-Timer)|AE_AML_LOOP_TIMEOUT|clocksource: Switched/.test(line)){
      optionalObservations.push({...receipt,line:line.slice(-2048)});
    }
  }
};
try{
  console.log('NOIRQ_BOOT_DIAGNOSTIC '+JSON.stringify({phase:'observation_launch',sourceCommit,
    diagnosticOnly:true,acceptanceVerified:false,maxVmStarts:1,hostObservationMilliseconds:300000,jobBudget}));
  const launch=firefox.launch({headless:true,timeout:Math.min(30000,remaining())});
  launch.then(value=>{if(observationEnded)void value.close().catch(()=>{});},()=>{});
  browser=await wait(launch);
  browserVersion=await wait(browser.version());
  assert.equal(browserVersion,'159.0','The retained comparison uses locked Firefox159');
  const context=await wait(browser.newContext());
  page=await wait(context.newPage());
  page.on('pageerror',recordFailure);
  page.on('crash',()=>recordFailure(new Error('Browser page crashed')));
  await wait(page.goto(url,{timeout:Math.min(30000,remaining())}));
  await wait(page.waitForFunction(()=>crossOriginIsolated&&!document.getElementById('start').disabled,
    null,{timeout:Math.min(30000,remaining())}));
  await wait(page.exposeBinding('__recordBrowserLabLog',(_source,batch)=>{
    try{
      assert(receipts.length<100000,'Host receipt timeline exceeded its explicit bound');
      guestLog.appendBatch(batch);
      const receipt={firstSequence:batch[0].sequence,chunks:batch.length,
        hostElapsedMilliseconds:performance.now()-startedMono,receivedAt:new Date().toISOString()};
      receipts.push([receipt.firstSequence,receipt.chunks,receipt.hostElapsedMilliseconds]);
      observe(batch,receipt);
    }catch(error){recordFailure(error);throw error;}
  }));
  await wait(page.exposeBinding('__reportBrowserLabLogFailure',(_source,message)=>recordFailure(new Error(message))));
  await wait(page.evaluate(async()=>{
    const {subscribeToGuestLog}=await import('./log-stream.mjs');
    window.__finishBrowserLabLog=subscribeToGuestLog(window,batch=>window.__recordBrowserLabLog(batch),
      {onError:error=>window.__reportBrowserLabLogFailure(String(error))});
  }));
  assert.deepEqual(await wait(page.evaluate(()=>({sequence:window.labLogSequence,accepted:window.labEvidence.accepted,
    checks:window.labEvidence.checks}))),{sequence:0,accepted:false,checks:0});
  vmStartRequested=true;
  await wait(page.locator('#start').click({timeout:Math.min(30000,remaining())}));
  while(true){
    const state=await wait(page.evaluate(()=>({booted:window.labEvidence.booted,error:window.labEvidence.error})));
    if(state.error)throw new Error(state.error);
    if(state.booted&&Object.keys(patterns).every(name=>markers[name])){
      assert(performance.now()<=hostDeadline,'Boot markers arrived after the host deadline');
      bootObserved=true;
      break;
    }
    await wait(new Promise(resolve=>setTimeout(resolve,250)));
  }
}catch(error){failure=String(error);}
finally{
  observationEnded=true;
  observationElapsedMilliseconds=performance.now()-startedMono;
  clearTimeout(deadlineTimer);
  // Capture cutoff, original guest state and actual arguments in one bounded
  // page operation. Preserve cutoff even when its pending drain then fails.
  if(page){
    const captureDeadline=performance.now()+10000;
    const captureRemaining=()=>Math.max(0,captureDeadline-performance.now());
    cutoff=await drainGuestLogWithin(page.evaluate(async()=>{
      const {freezeGuestLogCapture}=await import('./log-stream.mjs');
      const result=freezeGuestLogCapture(window);
      return {...result,actualArguments:Array.isArray(window.Module?.arguments)?window.Module.arguments.slice():null};
    }),captureRemaining()).catch(error=>({error:String(error)}));
    actualArguments=cutoff.actualArguments??null;
    if(!cutoff.error){
      const drained=await drainGuestLogWithin(page.evaluate(()=>window.__browserLabLogDrain),captureRemaining())
        .catch(error=>({error:String(error)}));
      cutoff={...cutoff,...drained};
    }
  }else cutoff={error:'No browser page was available for capture cutoff'};
  capture=guestLog.snapshot(cutoff.expectedSequence);
  if(cutoff.error||cutoff.deliveredSequence!==cutoff.expectedSequence)capture.metadata.complete=false;
  if(cutoff.error)capture.metadata.error=cutoff.error;
  try{if(browser)await drainGuestLogWithin(browser.close(),10000);}
  catch(error){cleanupError=String(error);}
  server.closeAllConnections();
  await drainGuestLogWithin(new Promise(resolve=>server.close(resolve)),1000)
    .catch(error=>{cleanupError??=String(error);});
}
let validationError=null;
try{
  assert.equal(failure,null);
  assert.equal(cleanupError,null);
  assert.equal(bootObserved,true);
  assert.equal(scanIndeterminate,false,'A marker line exceeded the bounded observer');
  assert.deepEqual(errors,[]);
  assert.equal(capture.metadata.complete,true);
  assert.equal(cutoff.guest?.error,undefined);
  assert.equal(cutoff.guest?.booted,true);
  assert.deepEqual(actualArguments,materialization.expectedArguments);
  assert.equal(cutoff.guest.vm.cpus,2);
  assert.equal(cutoff.guest.vm.cpuTopology,'2,sockets=2,cores=1,threads=1');
  assert.equal(cutoff.guest.vm.memoryMiB,512);
  assert.equal(cutoff.guest.vm.qemuLiveInstanceLimit,8192);
  assert(Number.isSafeInteger(cutoff.guest.vm.reportedLogicalCPUs)&&cutoff.guest.vm.reportedLogicalCPUs>=2);
  assert.deepEqual(cutoff.guest.startupManifest,startup);
  for(const name of Object.keys(patterns))assert(markers[name],name+' was not confirmed');
}catch(error){validationError=String(error);}
const files={};
for(const [name,text] of Object.entries({
  'firefox-console.txt':capture.text,'firefox-stream.jsonl':capture.jsonl,
  'firefox-receipts.json':JSON.stringify({format:'host-batch-receipts-v1',
    columns:['firstSequence','chunks','hostElapsedMilliseconds'],records:receipts},null,2)+'\n'
})){
  await fs.writeFile(path.join(evidenceDirectory,name),text);
  files[name]={bytes:Buffer.byteLength(text),sha256:digest(text)};
}
const observedLines=capture.text.split(/\r?\n/).filter(line=>
  /LAB\||\[browser-guest\]|ACPI|HPET|hpet|PM-Timer|clocksource|rootfs|Mounted root|MultiProcessor|PCI->APIC|ERROR|Error:|FATAL|PANIC/.test(line));
const summary={
  schemaVersion:1,experiment:'firefox_acpi_noirq_boot',diagnosticOnly:true,productionAcceptance:false,
  acceptanceVerified:false,sourceCommit,workflowCommit:process.env.GITHUB_SHA,engine:'firefox',
  startedAt,finishedAt:new Date().toISOString(),browser:browserVersion,vmStartRequested,maxVmStarts:1,
  hostObservationMilliseconds:300000,observationElapsedMilliseconds,bootObserved:validationError===null,
  stopReason:failure??(bootObserved?'required_boot_markers_observed':'unconfirmed_boot'),
  failure,validationError,cleanupError,pageErrors:errors,guest:cutoff.guest??null,
  actualArguments,markers,scanIndeterminate,optionalObservations,
  logCapture:capture.metadata,receiptBatches:receipts.length,files,jobBudget,
  configurationExperiment:materialization.configurationExperiment,
  materialization:{sourceCommit,data:materialization.data,loader:materialization.loader,
    kernel:materialization.kernel,kernelConfig:materialization.kernelConfig,
    rootfsUnchanged:materialization.rootfsUnchanged,emulator:materialization.emulator,
    openfgaBinary:materialization.openfgaBinary,bootstrapBinary:materialization.nativeVerification.bootstrapBinary},
  harnessSha256:digest(await fs.readFile(path.resolve(process.argv[1]))),
  logModuleSha256:digest(await fs.readFile(logModule)),
  observationLimit:'Positive boot markers only. Combined UART and emulator print output can interleave; missing markers do not prove absent kernel events.',
  excerpt:observedLines.slice(-40).map(line=>line.slice(0,1500))
};
await fs.writeFile(path.join(evidenceDirectory,'firefox-summary.json'),JSON.stringify(summary,null,2)+'\n');
const compactGuest=summary.guest?{booted:summary.guest.booted,ready:summary.guest.ready,
  accepted:summary.guest.accepted,checks:summary.guest.checks,error:summary.guest.error??null,vm:summary.guest.vm}:null;
console.log('NOIRQ_BOOT_DIAGNOSTIC '+JSON.stringify({...summary,guest:compactGuest}));
if(validationError!==null)process.exitCode=1;
if(cleanupError!==null)process.exit(1);
