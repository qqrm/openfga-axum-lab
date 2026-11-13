// Bounded kernel-boot observation only. No backend readiness or acceptance claim.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream,promises as fs} from 'node:fs';
import {createServer} from 'node:http';
import {createRequire} from 'node:module';
import {performance} from 'node:perf_hooks';
import path from 'node:path';
import {createGuestLogCollector,drainGuestLogWithin} from '../source-new/browser-single/pages/log-stream.mjs';

const require = createRequire(new URL('../source-new/e2e/package.json',import.meta.url));
const {firefox} = require('@playwright/test');
assert.equal(require('@playwright/test/package.json').version,'1.65.0');
const output = path.resolve('artifacts/firefox-kernel-comparison');
const preparedBytes = await fs.readFile(path.join(output,'prepared.json'));
const prepared = JSON.parse(preparedBytes);
assert.equal(prepared.diagnosticOnly,true);
assert.equal(prepared.productionAcceptance,false);
assert.deepEqual(prepared.prepared.map(x=>x.label),['old','new']);
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const sourceHash = digest(await fs.readFile(new URL(import.meta.url)));
assert.equal(prepared.runner.sha256,sourceHash);
const order = ['old','new','new','old'];
const limitMs = 180000;
const summary = {diagnosticOnly:true,productionAcceptance:false,
  scope:'fresh Firefox kernel boot of exact retained bundles; stops at LAB|boot or host deadline',
  order,hostLimitMsPerAttempt:limitMs,maxVMStarts:4,
  bundleMode:prepared.bundleMode,hostSourceHead:prepared.hostSourceHead,
  preparedSha256:digest(preparedBytes),runnerSha256:sourceHash,attempts:[]};
const types = {'.html':'text/html','.js':'text/javascript','.mjs':'text/javascript','.css':'text/css',
  '.wasm':'application/wasm','.json':'application/json','.sh':'text/plain'};
const write = (name,value) => fs.writeFile(path.join(output,name),JSON.stringify(value,null,2)+'\n');
function serverFor(root) {
  return createServer(async (request,response) => {
    try {
      const url = new URL(request.url,'http://localhost');
      const pathname = decodeURIComponent(url.pathname);
      const file = path.resolve(root,'.'+pathname+(pathname.endsWith('/')?'index.html':''));
      if (!file.startsWith(root+path.sep)) throw new Error('Outside fixture');
      const stat = await fs.stat(file);
      if (!stat.isFile()) throw new Error('Not a regular file');
      // Same isolation service worker as production; no extra isolation headers.
      response.writeHead(200,{'Content-Type':types[path.extname(file)]??'application/octet-stream',
        'Content-Length':stat.size});
      const stream = createReadStream(file);
      stream.on('error',()=>response.destroy());
      stream.pipe(response);
    } catch {response.writeHead(404);response.end();}
  });
}
function verifyCapture(captured) {
  let count=0,bytes=0,batches=0;
  const text=[];
  const encoder = new TextEncoder();
  for (const line of captured.jsonl.split('\n').filter(Boolean)) {
    const record=JSON.parse(line);
    assert.equal(record.firstSequence,count+1);
    assert(Array.isArray(record.texts) && record.texts.length>0);
    assert(Number.isFinite(Date.parse(record.receivedAt)));
    batches++;
    for (const chunk of record.texts) {
      assert.equal(typeof chunk,'string');
      count++;bytes+=encoder.encode(chunk).byteLength;text.push(chunk);
    }
  }
  assert.equal(text.join(''),captured.text);
  assert.equal(count,captured.metadata.chunks);
  assert.equal(bytes,captured.metadata.bytes);
  assert.equal(batches,captured.metadata.batches);
  return {reconstructedConsoleEqual:true,contiguousSequences:true,
    sourceChunks:count,batches,encodedChunkBytes:bytes,
    consoleUtf8Bytes:Buffer.byteLength(captured.text),
    consoleSha256:digest(Buffer.from(captured.text)),streamSha256:digest(Buffer.from(captured.jsonl))};
}
let cleanupFailed=false;
let expectedBrowser;
let expectedArgs;
try {
  for (const [index,label] of order.entries()) {
    const bundle=prepared.prepared.find(x=>x.label===label);
    const name=String(index+1).padStart(2,'0')+'-'+label;
    const evidence={diagnosticOnly:true,productionAcceptance:false,attempt:index+1,label,
      sourceHead:bundle.head,sourceArtifact:bundle.archived.artifact,
      expectedRuntime:bundle.files,startedAt:new Date().toISOString(),
      hostLimitMs:limitMs,vmStartRequested:false,bootObserved:false,pageErrors:[]};
    const log=createGuestLogCollector();
    const server=serverFor(bundle.root);
    await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
    const url='http://127.0.0.1:'+server.address().port+'/wasm/';
    let browser,page,timer,rejectPageFailure,fatalCaptureFailure=false;
    const started=performance.now();
    const hostTimeout=new Error('Diagnostic host deadline of 180000ms reached');
    const deadline=new Promise((_,reject)=>{timer=setTimeout(()=>reject(hostTimeout),limitMs);});
    deadline.catch(()=>{});
    const pageFailure=new Promise((_,reject)=>{rejectPageFailure=reject;});
    pageFailure.catch(()=>{});
    const failPage=value=>{
      const message=String(value);
      if(evidence.pageErrors.length<32) evidence.pageErrors.push(message);
      rejectPageFailure(value instanceof Error?value:new Error(message));
    };
    const wait=operation=>Promise.race([operation,deadline,pageFailure]);
    try {
      browser=await wait(firefox.launch({headless:true,timeout:30000}));
      evidence.browser=await browser.version();
      assert.equal(evidence.browser,'159.0','Unexpected pinned Firefox version');
      if(expectedBrowser===undefined) expectedBrowser=evidence.browser;
      assert.equal(evidence.browser,expectedBrowser,'Firefox version changed between attempts');
      const context=await wait(browser.newContext());
      page=await wait(context.newPage());
      page.on('pageerror',failPage);
      page.on('crash',()=>failPage('Browser page crashed'));
      await wait(page.goto(url));
      await wait(page.waitForFunction(()=>crossOriginIsolated && !document.getElementById('start').disabled,
        null,{timeout:30000}));
      evidence.browserEnvironment=await wait(page.evaluate(()=>({
        userAgent:navigator.userAgent,reportedLogicalCPUs:navigator.hardwareConcurrency,
        crossOriginIsolated,secureContext:isSecureContext
      })));
      // Both original CI runs reported four CPUs; reject a different comparison host.
      assert.equal(evidence.browserEnvironment.reportedLogicalCPUs,4);
      await wait(page.exposeBinding('__recordBrowserLabLog',(_source,batch)=>{
        try {log.appendBatch(batch);} catch(error) {failPage(error);throw error;}
      }));
      await wait(page.exposeBinding('__reportBrowserLabLogFailure',(_source,message)=>failPage(new Error(message))));
      await wait(page.evaluate(async()=>{
        const m=await import('./log-stream.mjs');
        window.__finishBrowserLabLog=m.subscribeToGuestLog(window,batch=>window.__recordBrowserLabLog(batch),
          {onError:error=>window.__reportBrowserLabLogFailure(String(error))});
        window.__freezeDiagnosticLog=()=>{
          const cutoff=m.freezeGuestLogCapture(window);
          return {...cutoff,phase:document.getElementById('phase').textContent,
            detail:document.getElementById('detail').textContent,
            arguments:Array.isArray(window.Module?.arguments)?[...window.Module.arguments]:null};
        };
      }));
      const initial=await wait(page.evaluate(()=>({sequence:window.labLogSequence,evidence:window.labEvidence})));
      assert.equal(initial.sequence,0);
      assert.equal(initial.evidence.booted,false);
      assert.equal(initial.evidence.accepted,false);
      assert.equal(initial.evidence.checks,0);
      evidence.vmStartRequested=true;
      evidence.vmStartRequestedAt=new Date().toISOString();
      await wait(page.locator('#start').click());
      await wait(page.waitForFunction(()=>window.labEvidence.booted || window.labEvidence.error,
        null,{timeout:limitMs}));
      const observed=await wait(page.evaluate(()=>window.labEvidence));
      assert.equal(observed.error,undefined,'Guest reported an error before diagnostic stop: '+String(observed.error));
      assert.equal(observed.booted,true);
      evidence.bootObserved=true;
      evidence.outcome='kernel_boot_observed';
    } catch(error) {
      evidence.outcome=error===hostTimeout?'host_deadline':'runtime_or_preflight_failure';
      evidence.failure=String(error);
      if(!evidence.vmStartRequested) fatalCaptureFailure=true;
    } finally {
      clearTimeout(timer);
      evidence.stopRequestedAt=new Date().toISOString();
      evidence.observationElapsedMs=Math.round(performance.now()-started);
      try {
        let cutoff={error:'Page unavailable at capture cutoff'};
        if(page) {
          const captureDeadline=performance.now()+10000;
          const remaining=()=>Math.max(0,captureDeadline-performance.now());
          cutoff=await drainGuestLogWithin(page.evaluate(()=>window.__freezeDiagnosticLog()),remaining())
            .catch(error=>({error:String(error)}));
          if(!cutoff.error) {
            const drained=await drainGuestLogWithin(page.evaluate(()=>window.__browserLabLogDrain),remaining())
              .catch(error=>({error:String(error)}));
            cutoff={...cutoff,...drained};
          }
        }
        evidence.guest=cutoff.guest??null;
        evidence.phase=cutoff.phase??null;
        evidence.detail=cutoff.detail??null;
        evidence.actualArguments=cutoff.arguments??null;
        const captured=log.snapshot(cutoff.expectedSequence);
        evidence.logCapture={...captured.metadata,...(cutoff.error?{error:cutoff.error}:{})};
        if(cutoff.error || cutoff.deliveredSequence!==cutoff.expectedSequence) evidence.logCapture.complete=false;
        await fs.writeFile(path.join(output,name+'-console.txt'),captured.text);
        await fs.writeFile(path.join(output,name+'-stream.jsonl'),captured.jsonl);
        evidence.captureIntegrity=verifyCapture(captured);
        if(evidence.bootObserved && (!/^LAB\|boot\r?$/m.test(captured.text) ||
            !evidence.guest?.booted || evidence.guest?.error || evidence.pageErrors.length)) {
          evidence.outcome='invalid_or_failed_boot_observation';
        }
        if(!evidence.logCapture.complete) {
          evidence.outcome='incomplete_capture';
          fatalCaptureFailure=true;
        }
        if(evidence.guest?.startupManifest) {
          assert.deepEqual(evidence.guest.startupManifest,
            JSON.parse(await fs.readFile(path.join(bundle.root,'wasm/runtime/startup-manifest.json'),'utf8')),
            'Actual startup manifest differs from retained bundle');
        }
        if(evidence.guest?.vm?.qemuLiveInstanceLimit!=null) {
          assert.equal(evidence.guest.vm.qemuLiveInstanceLimit,8192);
        }
        if(evidence.actualArguments) {
          if(expectedArgs===undefined) expectedArgs=evidence.actualArguments;
          assert.deepEqual(evidence.actualArguments,expectedArgs,'Guest arguments changed between bundles');
        }
      } catch(error) {
        fatalCaptureFailure=true;
        evidence.captureFailure=String(error);
        if(evidence.logCapture) evidence.logCapture.complete=false;
        evidence.outcome='harness_integrity_failure';
      } finally {
        // Never start another browser if the previous process failed to close.
        if(browser) {
          try {await drainGuestLogWithin(browser.close(),10000);}
          catch(error) {evidence.cleanupFailure=String(error);cleanupFailed=true;}
        }
        if(evidence.bootObserved && evidence.pageErrors.length) evidence.outcome='kernel_boot_then_page_failure';
        server.closeAllConnections();
        try {await drainGuestLogWithin(new Promise(resolve=>server.close(resolve)),1000);}
        catch(error) {evidence.serverCleanupFailure=String(error);cleanupFailed=true;}
        evidence.finishedAt=new Date().toISOString();
        await write(name+'.json',evidence);
        summary.attempts.push({attempt:evidence.attempt,label,sourceHead:bundle.head,browser:evidence.browser,
          vmStartRequested:evidence.vmStartRequested,bootObserved:evidence.bootObserved,outcome:evidence.outcome,
          observationElapsedMs:evidence.observationElapsedMs,captureComplete:evidence.logCapture?.complete===true,
          pageErrors:evidence.pageErrors,captureFailure:evidence.captureFailure,
        cleanupFailure:evidence.cleanupFailure,serverCleanupFailure:evidence.serverCleanupFailure});
        await write('summary.json',summary);
        console.log('DIAGNOSTIC_ONLY '+JSON.stringify(summary.attempts.at(-1)));
      }
    }
    if(fatalCaptureFailure) throw new Error('Stopped remaining attempts after a preflight or integrity failure');
    if(cleanupFailed) throw new Error('Stopped remaining attempts after browser/server cleanup failure');
  }
  summary.allFourAttemptsRecorded=summary.attempts.length===4;
  summary.allCapturesComplete=summary.attempts.every(x=>x.captureComplete);
  await write('summary.json',summary);
  assert(summary.allCapturesComplete,'At least one diagnostic capture is incomplete');
} catch(error) {
  summary.harnessFailure=String(error);
  await write('summary.json',summary);
  process.exitCode=1;
}
