// One real browser VM, one isolated OpenFGA component workload. No app acceptance.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createReadStream, promises as fs} from 'node:fs';
import path from 'node:path';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import {git,record,verifyBuild} from './candidate-emulator.mjs';
import {createComponentRecordParser,validateComponentEvents} from './memory-component-protocol.mjs';

const sourceCommit = '444b069daf16e9606a0bbf1ea0aea7f4d61b97d8';
const emulatorCommit = '6f35f24b7ca345cf47c873becee6a41ef94e65a9';
const source = path.resolve('source-candidate');
const root = path.join(source,'browser-output');
const wasmRoot = path.join(root,'wasm');
const output = path.resolve('artifacts/pr36-memory-component');
const readJSON = async file=>JSON.parse(await fs.readFile(file,'utf8'));
await fs.mkdir(output,{recursive:true});
assert.equal(process.env.BROWSER_DIAGNOSTIC_ENGINE,'chromium');
assert.equal(process.env.BROWSER_DIAGNOSTIC_VCPUS,'2');
assert.equal(process.env.GITHUB_RUN_ATTEMPT,'1');
assert.equal(git(source,'rev-parse','HEAD'),sourceCommit);
assert.equal(git(path.resolve('emulator-source'),'rev-parse','HEAD'),emulatorCommit);
git(source,'diff','--exit-code','HEAD','--');

const materialization = await readJSON('artifacts/openfga10-bfs-browser-gate/materialization.json');
const emulator = await readJSON('artifacts/pr36-idle-gc-gate/emulator-overlay.json');
const topology = await readJSON('artifacts/pr36-vcpu-gate/topology-overlay.json');
const component = await readJSON(path.join(output,'component-overlay.json'));
assert.equal(materialization.sourceCommit,sourceCommit);
assert.equal(emulator.kind,'verified_emulator_only_overlay');
assert.equal(emulator.sourceIdentity.candidateSource,emulatorCommit);
assert.equal(emulator.all17NativeInputsUnchanged,true);
assert.equal(topology.kind,'verified_guest_vcpu_topology_overlay');
assert.equal(topology.requestedVirtualCPUs,2);
assert.equal(topology.controlBytesUnchanged,true);
assert.equal(topology.all17NativeInputsUnchanged,true);
assert.equal(component.kind,'verified_openfga_memory_component_overlay');
assert.equal(component.baselineSource,sourceCommit);
for (const key of ['all17NativeInputsUnchanged','applicationBinariesUnchanged','rootfsUnchanged','componentOnly']) {
  assert.equal(component[key],true,'Missing preserved component invariant: '+key);
}
assert.equal(component.authorizationDeadlinesChanged,false);
assert.equal(component.productionAcceptance,false);
assert.deepEqual(component.nativeInputs,emulator.nativeInputs);
const candidateBuild = await verifyBuild(path.resolve('emulator-source'),path.resolve('candidate-emulator'));
assert.deepEqual(candidateBuild,emulator.candidateEmulator);
for (const [name,expected] of Object.entries(candidateBuild.files)) {
  assert.deepEqual(await record(path.join(wasmRoot,'runtime',name)),expected);
}
for (const [name,expected] of Object.entries(component.preservedFiles)) {
  assert.deepEqual(await record(path.join(wasmRoot,name)),expected,'Preserved served file differs: '+name);
}
for (const [name,expected] of Object.entries(component.afterFiles)) {
  assert.deepEqual(await record(path.join(wasmRoot,name)),expected,'Component served file differs: '+name);
}
assert.deepEqual(await record(path.join(wasmRoot,'guest.sh')),component.componentGuest);
assert.deepEqual(await record(path.join(wasmRoot,'runtime/startup-manifest.json')),component.startupManifest);
assert.deepEqual(await record(path.join(wasmRoot,'runtime/derived-runtime.json')),component.derivedRuntime);
assert.deepEqual(await fs.readFile(path.join(wasmRoot,'launcher.js')),await fs.readFile(path.join(source,'browser-single/pages/launcher.js')));
const smoke = await readJSON('artifacts/pr36-vcpu-gate/chromium/system-chrome-smoke.json');
assert.equal(smoke.passed,true);
assert.equal(smoke.launchOptions.chromiumSandbox,true);
assert.equal(smoke.launchOptions.channel,'chrome');
assert.deepEqual(await record(smoke.executable.path),{bytes:smoke.executable.bytes,sha256:smoke.executable.sha256});
const timing = await readJSON('prepared/job-timing.json');
assert.equal(timing.source,'GitHub Actions current job API');
assert.equal(timing.name,'OpenFGA memory browser component');
assert.equal(timing.run_id,Number(process.env.GITHUB_RUN_ID));
assert.equal(timing.head_sha,process.env.GITHUB_SHA);
assert.equal(timing.run_attempt,1);
assert(Number.isSafeInteger(timing.id) && timing.id>0);
const jobElapsed = Date.now()-Date.parse(timing.started_at);
const budget = {...timing,checkedAt:new Date().toISOString(),jobElapsedMilliseconds:jobElapsed,
  latestLaunchJobElapsedMilliseconds:480000,outerJobMinutes:35,runtimeStepMinutes:22,
  componentWaitMilliseconds:1200000,launchPermitted:Number.isFinite(jobElapsed)&&jobElapsed>=0&&jobElapsed<=480000};
await fs.writeFile(path.join(output,'job-budget.json'),JSON.stringify(budget,null,2)+'\n');
if (!budget.launchPermitted) {
  await fs.writeFile(path.join(output,'preflight-stop.json'),JSON.stringify({reason:'Insufficient or unverified remaining Actions job budget',vmStarts:0,productionAcceptance:false,budget},null,2)+'\n');
  throw new Error('Refusing to launch the VM more than eight minutes after the actual job start');
}

const require = createRequire(path.join(source,'e2e/package.json'));
const {chromium} = require('@playwright/test');
const {createGuestLogCollector,drainGuestLogWithin} = await import(pathToFileURL(path.join(source,'browser-single/pages/log-stream.mjs')).href);
const types = {'.html':'text/html','.js':'text/javascript','.mjs':'text/javascript','.css':'text/css','.wasm':'application/wasm','.json':'application/json','.sh':'text/plain'};
const server = createServer(async (request,response)=>{
  try {
    const url = new URL(request.url,'http://localhost');
    const file = path.resolve(root,`.${decodeURIComponent(url.pathname)}${url.pathname.endsWith('/')?'index.html':''}`);
    if (!file.startsWith(root+path.sep)) throw new Error('Path outside fixture');
    const stat = await fs.stat(file);
    if (!stat.isFile()) throw new Error('Not a regular file');
    response.writeHead(200,{'Content-Type':types[path.extname(file)]||'application/octet-stream','Content-Length':stat.size});
    createReadStream(file).pipe(response);
  } catch { response.writeHead(404);response.end(); }
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const url = `http://127.0.0.1:${server.address().port}/wasm/`;
const evidence = {kind:'real_openfga_memory_browser_component',engine:'chromium',sourceCommit,
  emulatorSource:emulatorCommit,workflowCommit:process.env.GITHUB_SHA,runId:Number(process.env.GITHUB_RUN_ID),
  startedAt:new Date().toISOString(),componentPassed:false,productionAcceptance:false,vmStarts:0,
  jobBudget:budget,componentOverlay:await record(path.join(output,'component-overlay.json')),
  captureImplementation:await record(path.join(source,'browser-single/pages/log-stream.mjs')),
  events:[],pageErrors:[]};
const guestLog = createGuestLogCollector();
const componentLog = createComponentRecordParser();
let browser,page,hostTimer,heartbeat;
let resolveTerminal,rejectFailure,resolveFailureDiagnostics;
const terminal = new Promise(resolve=>{resolveTerminal=resolve;});
const failureDiagnostics = new Promise(resolve=>{resolveFailureDiagnostics=resolve;});
const failure = new Promise((_resolve,reject)=>{rejectFailure=reject;});
failure.catch(()=>{});
const pageFailure = error=>{
  evidence.pageErrors.push(String(error));
  rejectFailure(error instanceof Error?error:new Error(String(error)));
};
const wait = promise=>Promise.race([promise,failure]);
const consume = batch=>{
  guestLog.appendBatch(batch);
  for (const chunk of batch) {
    for (const parsed of componentLog.append(chunk)) {
      const event={...parsed,hostReceivedAt:new Date().toISOString()};
      evidence.events.push(event);
      assert(evidence.events.length<=64,'Unexpected component event count');
      console.log('FGA_MEMORY_HOST '+JSON.stringify({event:event.event,cycle:event.data.cycle,operation:event.data.operation,
        reason:event.data.reason,phase:event.data.phase,transportExit:event.data.transport_exit}));
      if (event.event==='done'||event.event==='failed') resolveTerminal(event);
      if (event.event==='stage'&&event.data.phase==='failure-diagnostics-finished') resolveFailureDiagnostics();
    }
  }
};
try {
  browser=await chromium.launch({headless:true,channel:'chrome',chromiumSandbox:true,args:['--enable-automation']});
  evidence.browser=browser.version();
  const context=await browser.newContext();
  page=await context.newPage();
  page.on('pageerror',pageFailure);
  page.on('crash',()=>pageFailure(new Error('Browser page crashed')));
  await wait(page.goto(url));
  await wait(page.waitForFunction(()=>crossOriginIsolated&&!document.getElementById('start').disabled,null,{timeout:30000}));
  await page.exposeBinding('__recordBrowserLabLog',(_source,batch)=>{
    try { consume(batch); } catch(error) { pageFailure(error);throw error; }
  });
  await page.exposeBinding('__reportBrowserLabLogFailure',(_source,message)=>pageFailure(new Error(message)));
  await page.evaluate(async()=>{
    const {subscribeToGuestLog}=await import('./log-stream.mjs');
    window.__finishBrowserLabLog=subscribeToGuestLog(window,batch=>window.__recordBrowserLabLog(batch),
      {onError:error=>window.__reportBrowserLabLogFailure(String(error))});
  });
  assert.equal(await page.evaluate(()=>window.labLogSequence),0);
  assert.equal(await page.evaluate(()=>window.labEvidence.accepted),false);
  evidence.vmStarts=1;
  hostTimer=setTimeout(()=>rejectFailure(new Error('Component host wait exceeded 1200000 ms')),1200000);
  heartbeat=setInterval(()=>console.log('FGA_MEMORY_HOST '+JSON.stringify({event:'running',elapsedMs:Date.now()-Date.parse(evidence.startedAt),events:evidence.events.length})),60000);
  await wait(page.locator('#start').click());
  const pageRuntimeFailure=page.waitForFunction(()=>window.labEvidence.error,null,{timeout:0}).then(async handle=>{
    throw new Error('Guest runtime failed: '+await handle.jsonValue());
  });
  pageRuntimeFailure.catch(()=>{});
  const last=await wait(Promise.race([terminal,pageRuntimeFailure]));
  if (last.event==='failed') await drainGuestLogWithin(failureDiagnostics,10000).catch(()=>{});
  assert.equal(last.event,'done','Guest component failure: '+JSON.stringify(last.data));
  evidence.verified=validateComponentEvents(evidence.events);
  evidence.componentPassed=true;
} catch(error) {
  evidence.failure=String(error);
  console.error('OpenFGA memory component failed: '+error);
} finally {
  clearTimeout(hostTimer);clearInterval(heartbeat);
  if (page) {
    const deadline=Date.now()+10000;
    const remaining=()=>Math.max(0,deadline-Date.now());
    let cutoff=await drainGuestLogWithin(page.evaluate(async()=>{
      const {freezeGuestLogCapture}=await import('./log-stream.mjs');
      return freezeGuestLogCapture(window);
    }),remaining()).catch(error=>({error:String(error)}));
    if (!cutoff.error) {
      const drain=await drainGuestLogWithin(page.evaluate(()=>window.__browserLabLogDrain),remaining()).catch(error=>({error:String(error)}));
      cutoff={...cutoff,...drain};
    }
    evidence.guest=cutoff.guest??{error:cutoff.error||'Guest state unavailable at capture cutoff'};
    const captured=guestLog.snapshot(cutoff.expectedSequence);
    evidence.logCapture={...captured.metadata,...(cutoff.error?{error:cutoff.error}:{})};
    if (cutoff.error||cutoff.deliveredSequence!==cutoff.expectedSequence) evidence.logCapture.complete=false;
    await fs.writeFile(path.join(output,'chromium-console.txt'),captured.text);
    await fs.writeFile(path.join(output,'chromium-stream.jsonl'),captured.jsonl);
    await page.screenshot({path:path.join(output,'chromium.png'),fullPage:true,timeout:10000}).catch(error=>{evidence.screenshotError=String(error);});
  }
  evidence.componentCapture=componentLog.snapshot();
  if (evidence.componentPassed) {
    try {
      evidence.verified=validateComponentEvents(evidence.events);
      assert.equal(evidence.logCapture?.complete,true,'Incomplete source capture');
      assert.equal(evidence.componentCapture.chunks,evidence.logCapture.chunks,'Component capture did not consume the full source prefix');
      assert.equal(evidence.componentCapture.partialComponentRecord,false,'Truncated component record at capture cutoff');
      assert.deepEqual(evidence.pageErrors,[]);
      assert.equal(evidence.guest?.error,undefined,'Runtime error at capture cutoff');
      assert.equal(evidence.guest?.vm?.cpus,2);
      assert.equal(evidence.guest?.vm?.cpuTopology,'2,sockets=2,cores=1,threads=1');
      assert.equal(evidence.guest?.accepted,false,'Component must never claim full app acceptance');
      assert.equal(evidence.guest?.checks,0,'Component must never increment full app acceptance');
    } catch(error) { evidence.componentPassed=false;evidence.failure=String(error); }
  }
  evidence.finishedAt=new Date().toISOString();
  await fs.writeFile(path.join(output,'chromium.json'),JSON.stringify(evidence,null,2)+'\n');
  const files={};
  for (const name of ['chromium-console.txt','chromium-stream.jsonl','chromium.json']) {
    try { files[name]=await record(path.join(output,name)); } catch(error) {files[name]={error:String(error)};}
  }
  await fs.writeFile(path.join(output,'summary.json'),JSON.stringify({kind:evidence.kind,sourceCommit,emulatorSource:emulatorCommit,
    workflowCommit:process.env.GITHUB_SHA,runId:Number(process.env.GITHUB_RUN_ID),componentPassed:evidence.componentPassed,
    productionAcceptance:false,vmStarts:evidence.vmStarts,files,job:timing,failure:evidence.failure},null,2)+'\n');
  await browser?.close();
  server.closeAllConnections();await new Promise(resolve=>server.close(resolve));
}
console.log('FGA_MEMORY_HOST '+JSON.stringify({event:'finished',componentPassed:evidence.componentPassed,productionAcceptance:false,failure:evidence.failure}));
if (!evidence.componentPassed) process.exitCode=1;
