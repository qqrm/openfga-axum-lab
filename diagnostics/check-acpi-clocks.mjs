// Diagnostic only: one retained real VM per engine, two vCPUs and pinned ACPI kernel; unchanged request budgets.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream,promises as fs} from 'node:fs';
import {createServer} from 'node:http';
import {createRequire} from 'node:module';
import {performance} from 'node:perf_hooks';
import path from 'node:path';
import {createGuestLogCollector,drainGuestLogWithin} from '../source-current/browser-single/pages/log-stream.mjs';
import {expectedStatuses} from '../source-current/browser-single/pages/acceptance-protocol.mjs';

const require=createRequire(new URL('../source-current/e2e/package.json',import.meta.url));
const {chromium,firefox}=require('@playwright/test');
assert.equal(require('@playwright/test/package.json').version,'1.65.0');
const selectedEngine=process.env.BROWSER_DIAGNOSTIC_ENGINE;
assert(['chromium','firefox'].includes(selectedEngine),'BROWSER_DIAGNOSTIC_ENGINE must select exactly one engine');
const output=path.resolve('artifacts/browser-clocks-acpi');
const preparedBytes=await fs.readFile(path.join(output,'prepared.json'));
const prepared=JSON.parse(preparedBytes);
assert.equal(prepared.diagnosticOnly,true);
assert.equal(prepared.productionAcceptance,false);
assert.deepEqual(prepared.prepared.map(x=>x.label),['current']);
assert.equal(prepared.hostSourceHead,'af9c5555e312ffaec3785154f5be83be7469c95f');
assert.equal(prepared.runtimeSourceHead,'379e3280c2a1c789071292af141648eb2fc3ffbc');
assert(prepared.kernelPin&&prepared.rootfsProof?.equal,'Missing reviewed kernel/rootfs materialization proof');
assert.deepEqual(prepared.rootfsProof.original,prepared.rootfsProof.retained);
assert.deepEqual(prepared.rootfsProof.original,prepared.rootfsProof.derived);
const bundle=prepared.prepared[0];
assert.equal(bundle.head,prepared.runtimeSourceHead);
assert.equal(bundle.hostHead,prepared.hostSourceHead);
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
const runnerSha256=digest(await fs.readFile(new URL(import.meta.url)));
assert.equal(prepared.runner.sha256,runnerSha256);
assert.equal(prepared.sourceLogModule.sha256,
  digest(await fs.readFile('source-current/browser-single/pages/log-stream.mjs')));
const expectedStartup=JSON.parse(await fs.readFile(path.join(bundle.root,'wasm/runtime/startup-manifest.json'),'utf8'));
assert.equal(expectedStartup.diagnostic.requested_vcpus,2);
assert.equal(expectedStartup.diagnostic.acpi_discovery_enabled,true);
assert.equal(expectedStartup.diagnostic.host_source_commit,prepared.hostSourceHead);
assert.equal(expectedStartup.diagnostic.runtime_source_commit,prepared.runtimeSourceHead);
assert.deepEqual(expectedStartup.diagnostic.kernel,{artifact:prepared.kernelPin.artifact,
  archive_sha256:prepared.kernelPin.archive.sha256,bzImage:prepared.kernelPin.files.bzImage});
assert.equal(expectedStartup.diagnostic.productionAcceptance,false);
const limitMs=1200000;
const summary={diagnosticOnly:true,productionAcceptance:false,
  scope:'two-vCPU ACPI kernel clock observation; original two real authorization sequences if ready',
  order:[selectedEngine],hostLimitMsPerEngine:limitMs,maxVMStarts:1,
  workflowEngineMatrix:['chromium','firefox'],independentRunnerJobs:true,
  bundleMode:prepared.bundleMode,hostSourceHead:prepared.hostSourceHead,runtimeSourceHead:prepared.runtimeSourceHead,
  kernelArtifact:prepared.kernelPin.artifact,kernelArchiveSha256:prepared.kernelPin.archive.sha256,
  preparedSha256:digest(preparedBytes),runnerSha256,attempts:[]};
const types={'.html':'text/html','.js':'text/javascript','.mjs':'text/javascript','.css':'text/css',
  '.wasm':'application/wasm','.json':'application/json','.sh':'text/plain'};
const write=(name,value)=>fs.writeFile(path.join(output,name),JSON.stringify(value,null,2)+'\n');
function serverFor(root) {
  return createServer(async(request,response)=>{
    try {
      const pathname=decodeURIComponent(new URL(request.url,'http://localhost').pathname);
      const file=path.resolve(root,'.'+pathname+(pathname.endsWith('/')?'index.html':''));
      if(!file.startsWith(root+path.sep)) throw new Error('Outside fixture');
      const stat=await fs.stat(file);
      if(!stat.isFile()) throw new Error('Not a regular file');
      // Exercise the unchanged production isolation service worker.
      response.writeHead(200,{'Content-Type':types[path.extname(file)]??'application/octet-stream',
        'Content-Length':stat.size});
      const stream=createReadStream(file);stream.on('error',()=>response.destroy());stream.pipe(response);
    } catch {response.writeHead(404);response.end();}
  });
}
function verifyCapture(captured,timeline) {
  let count=0,bytes=0,batches=0,lastMs=-1;
  const text=[],encoder=new TextEncoder();
  for(const line of captured.jsonl.split('\n').filter(Boolean)) {
    const record=JSON.parse(line),receipt=timeline[batches];
    assert.equal(record.firstSequence,count+1);
    assert(Array.isArray(record.texts)&&record.texts.length>0);
    assert(Number.isFinite(Date.parse(record.receivedAt)));
    assert.deepEqual(receipt?.slice(0,2),[record.firstSequence,record.texts.length]);
    assert(Number.isFinite(receipt[2])&&receipt[2]>=lastMs);lastMs=receipt[2];batches++;
    for(const chunk of record.texts) {
      assert.equal(typeof chunk,'string');count++;bytes+=encoder.encode(chunk).byteLength;text.push(chunk);
    }
  }
  assert.equal(text.join(''),captured.text);
  assert.equal(count,captured.metadata.chunks);
  assert.equal(bytes,captured.metadata.bytes);
  assert.equal(batches,captured.metadata.batches);
  assert.equal(batches,timeline.length);
  return {reconstructedConsoleEqual:true,contiguousSequences:true,receiptMapComplete:true,
    sourceChunks:count,batches,encodedChunkBytes:bytes,consoleUtf8Bytes:Buffer.byteLength(captured.text),
    consoleSha256:digest(Buffer.from(captured.text)),streamSha256:digest(Buffer.from(captured.jsonl))};
}
function hostReceipt(timeline,sequence) {
  let low=0,high=timeline.length;
  while(low<high) {
    const middle=(low+high)>>>1,record=timeline[middle];
    if(record[0]+record[1]<=sequence) low=middle+1;else high=middle;
  }
  const record=timeline[low];
  assert(record&&sequence>=record[0]&&sequence<record[0]+record[1],'Observer row lacks a captured host receipt');
  return record[2];
}
function clockReport(observer,timeline,expectedSequence) {
  const result={clockCorrectness:'not_established',framingComplete:false,samples:[],intervals:[],issues:[],
    interpretation:'Host timestamps describe receipt of a batch, not exact guest execution. Compare elapsed intervals only; boot sets wall time from an earlier host value.'};
  if(!observer) {result.issues.push('No clean-PTY observer state');return result;}
  if(observer.clockError) result.issues.push(observer.clockError);
  const rows=observer.clockRows;
  if(!Array.isArray(rows)||rows.length>3000) {result.issues.push('Invalid bounded observer rows');return result;}
  let active=null;
  const seen=new Set();
  for(const row of rows) {
    try {
      assert(Number.isSafeInteger(row.endSequence)&&row.endSequence>0&&row.endSequence<=expectedSequence);
      assert(Number.isFinite(row.pageMonotonicMs));
      assert.equal(typeof row.text,'string');assert(row.text.length<=512);
      const received=hostReceipt(timeline,row.endSequence);
      const match=/^\[browser-clock\] (begin|time|source|available|event|intr|softirq|irq_status|end) id=(\d+)(?: (.*))?$/.exec(row.text);
      assert(match,'Malformed clean clock line');
      const [,kind,idText,fields='']=match,id=Number(idText);
      assert(Number.isSafeInteger(id)&&((id>=0&&id<=115)||(id>=1001&&id<=1004)),'Unexpected clock ID');
      if(kind==='begin') {
        if(active) result.issues.push('Incomplete frame before id='+id);
        assert(!seen.has(id),'Duplicate clock ID');seen.add(id);
        assert(/^point=[a-z-]+$/.test(fields),'Invalid clock point');
        active={id,point:fields.slice(6),beginSequence:row.endSequence,
          beginHostReceiptMs:received,events:[],softirqs:{}};
      } else {
        assert(active&&active.id===id,'Clock frame order mismatch');
        if(kind==='time') {
          assert(!active.time,'Duplicate clock time');
          const values=/^uptime=([0-9]+(?:\.[0-9]+)?) idle=([0-9]+(?:\.[0-9]+)?) epoch=([0-9]+(?:\.[0-9]+)?) seconds=([0-9]+)$/.exec(fields);
          assert(values,'Clock time unavailable or malformed');
          active.time={raw:fields,uptimeSeconds:Number(values[1]),idleSeconds:Number(values[2]),
            epochSeconds:Number(values[3]),shellSeconds:Number(values[4]),hostReceiptMs:received,
            endSequence:row.endSequence,pageMonotonicMs:row.pageMonotonicMs};
        } else if(kind==='source') {
          assert(active.source===undefined,'Duplicate clocksource');assert(fields.startsWith('value='));
          active.source=fields.slice(6);
          if(!active.source||active.source==='unavailable') result.issues.push('Clocksource unavailable for id='+id);
        } else if(kind==='available') {
          assert(active.available===undefined,'Duplicate available clocksources');assert(fields.startsWith('value='));
          active.available=fields.slice(6);
          if(!active.available||active.available==='unavailable') result.issues.push('Available clocksources missing for id='+id);
        } else if(kind==='event') {assert(active.events.length<4);active.events.push(fields);}
        else if(kind==='intr') {
          assert(active.intr===undefined,'Duplicate aggregate interrupt counter');
          const value=/^total=(\d+|unavailable)$/.exec(fields);assert(value,'Invalid aggregate interrupt row');
          const total=value[1]==='unavailable'?null:Number(value[1]);
          assert(total===null||Number.isSafeInteger(total));
          if(total===null) result.issues.push('Aggregate interrupt counter unavailable for id='+id);
          active.intr={total,hostReceiptMs:received,endSequence:row.endSequence};
        } else if(kind==='softirq') {
          const value=/^name=(TIMER|SCHED|RCU) value=(.*)$/.exec(fields);
          assert(value&&!Object.hasOwn(active.softirqs,value[1]),'Invalid or duplicate softirq row');
          const parsed=new RegExp('^\\s*'+value[1]+':\\s+(\\d+)\\s+(\\d+)\\s*$').exec(value[2]);
          const counts=parsed?[Number(parsed[1]),Number(parsed[2])]:null;
          assert(counts===null||counts.every(Number.isSafeInteger));
          if(counts===null) result.issues.push('Softirq counter unavailable or malformed for id='+id+' name='+value[1]);
          active.softirqs[value[1]]={raw:value[2],counts,hostReceiptMs:received,endSequence:row.endSequence};
        } else if(kind==='irq_status') {
          assert.equal(fields,'value=oci_masked_proc_interrupts');assert(active.irqStatus===undefined);
          active.irqStatus='oci_masked_proc_interrupts';
        } else {
          const counts=/^event_rows=(\d+) softirq_rows=3$/.exec(fields);
          assert(counts&&active.time&&active.source!==undefined&&active.available!==undefined&&
            active.intr!==undefined&&active.irqStatus==='oci_masked_proc_interrupts','Incomplete clock frame');
          assert.deepEqual(Object.keys(active.softirqs).sort(),['RCU','SCHED','TIMER']);
          assert.equal(Number(counts[1]),active.events.length);
          active.endSequence=row.endSequence;active.endHostReceiptMs=received;
          active.hostReceiptSpanMs=received-active.beginHostReceiptMs;
          result.samples.push(active);active=null;
        }
      }
    } catch(error) {result.issues.push(String(error));}
  }
  if(active) result.issues.push('Capture ended during clock frame id='+active.id);
  if(result.samples.length===0) result.issues.push('No complete clock observations');
  for(let index=1;index<result.samples.length;index++) {
    const a=result.samples[index-1],b=result.samples[index];
    result.intervals.push({fromId:a.id,toId:b.id,
      guestUptimeDeltaSeconds:b.time.uptimeSeconds-a.time.uptimeSeconds,
      guestEpochDeltaSeconds:b.time.epochSeconds-a.time.epochSeconds,
      shellDeltaSeconds:b.time.shellSeconds-a.time.shellSeconds,
      hostReceiptDeltaMs:b.time.hostReceiptMs-a.time.hostReceiptMs,
      sourceCounterDeltas:{intrTotal:a.intr.total===null||b.intr.total===null?null:b.intr.total-a.intr.total,
        softirqs:Object.fromEntries(['TIMER','SCHED','RCU'].map(name=>[name,
          a.softirqs[name].counts===null||b.softirqs[name].counts===null?null:
            b.softirqs[name].counts.map((value,cpu)=>value-a.softirqs[name].counts[cpu])]))}});
  }
  result.framingComplete=result.issues.length===0;
  result.totalRows=rows.length;
  result.interruptInterpretation='/proc/interrupts is OCI-masked; /proc/stat intr is an aggregate source counter, and TIMER/SCHED/RCU count softirq invocations. None directly counts awakened sleepers or proves clock correctness.';
  return result;
}
function assertPass(value,count,firstPass) {
  assert.equal(value.error,undefined,JSON.stringify(value));
  assert.equal(value.ready,true);assert.equal(value.accepted,true);
  assert.equal(value.checks,count);assert.equal(value.passes.length,count);
  assert.deepEqual(value.statuses,expectedStatuses);
  for(let index=0;index<count;index++) {
    assert.equal(value.passes[index].id,index+1);
    assert.equal(value.passes[index].accepted,true);
    assert.deepEqual(value.passes[index].statuses,expectedStatuses);
  }
  if(firstPass) assert.deepEqual(value.passes[0],firstPass);
}
function assertRuntime(guest,engine) {
  assert.deepEqual(guest.startupManifest,expectedStartup);
  assert.equal(guest.vm.cpus,2);assert.equal(guest.vm.cpuTopology,'2,sockets=2,cores=1,threads=1');
  assert.equal(guest.vm.memoryMiB,512);assert.equal(guest.vm.kernelLogLevel,7);
  assert(Number.isSafeInteger(guest.vm.reportedLogicalCPUs)&&guest.vm.reportedLogicalCPUs>=2);
  assert.equal(guest.vm.qemuLiveInstanceLimit,engine==='firefox'?8192:15000);
}
let cleanupFailed=false,expectedArgs;
try {
  for(const [engineName,engine] of [[selectedEngine,{chromium,firefox}[selectedEngine]]]) {
    const name=engineName,evidence={diagnosticOnly:true,productionAcceptance:false,engine:engineName,
      sourceHead:bundle.head,hostSourceHead:bundle.hostHead,sourceArtifact:bundle.archived.artifact,
      kernelArtifact:prepared.kernelPin.artifact,expectedRuntime:bundle.files,
      startedAt:new Date().toISOString(),hostLimitMs:limitMs,vmStartRequested:false,
      twoRealSequencesObserved:false,passes:[],pageErrors:[]};
    const log=createGuestLogCollector(),timeline=[],server=serverFor(bundle.root);
    await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
    const url='http://127.0.0.1:'+server.address().port+'/wasm/';
    let browser,page,timer,rejectPageFailure,fatalCaptureFailure=false;
    const started=performance.now(),hostTimeout=new Error('Diagnostic host deadline of 1200000ms reached');
    const deadline=new Promise((_,reject)=>{timer=setTimeout(()=>reject(hostTimeout),limitMs);});
    deadline.catch(()=>{});
    const pageFailure=new Promise((_,reject)=>{rejectPageFailure=reject;});pageFailure.catch(()=>{});
    const failPage=value=>{
      if(evidence.pageErrors.length<32) evidence.pageErrors.push(String(value));else evidence.pageErrorsTruncated=true;
      rejectPageFailure(value instanceof Error?value:new Error(String(value)));
    };
    const wait=operation=>Promise.race([operation,deadline,pageFailure]);
    try {
      browser=await wait(engine.launch({headless:true,timeout:30000}));
      evidence.browser=await browser.version();
      assert.equal(evidence.browser,engineName==='firefox'?'159.0':'158.0.8127.18','Unexpected locked browser version');
      const context=await wait(browser.newContext());page=await wait(context.newPage());
      page.on('pageerror',failPage);page.on('crash',()=>failPage('Browser page crashed'));
      await wait(page.goto(url));
      await wait(page.waitForFunction(()=>crossOriginIsolated&&!document.getElementById('start').disabled,null,{timeout:30000}));
      evidence.browserEnvironment=await wait(page.evaluate(()=>({
        userAgent:navigator.userAgent,reportedLogicalCPUs:navigator.hardwareConcurrency,
        crossOriginIsolated,secureContext:isSecureContext
      })));
      assert(Number.isSafeInteger(evidence.browserEnvironment.reportedLogicalCPUs)&&
        evidence.browserEnvironment.reportedLogicalCPUs>=2,'Conservative production host CPU preflight failed');
      await wait(page.exposeBinding('__recordBrowserLabLog',(_source,batch)=>{
        try {
          if(timeline.length>=1000000) throw new Error('Host receipt map exceeded its bounded batch count');
          const received=performance.now()-started;
          log.appendBatch(batch);timeline.push([batch[0].sequence,batch.length,received]);
        } catch(error) {failPage(error);throw error;}
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
            arguments:Array.isArray(window.Module?.arguments)?[...window.Module.arguments]:null,
            diagnostic:structuredClone(window.__browserClockDiagnostic)};
        };
      }));
      const initial=await wait(page.evaluate(()=>({sequence:window.labLogSequence,evidence:window.labEvidence})));
      assert.equal(initial.sequence,0);assert.equal(initial.evidence.booted,false);
      assert.equal(initial.evidence.accepted,false);assert.equal(initial.evidence.checks,0);
      evidence.vmStartRequested=true;evidence.vmStartRequestedAt=new Date().toISOString();
      console.log('DIAGNOSTIC_ONLY starting '+engineName+' with two vCPUs and pinned ACPI kernel; host deadline includes loading');
      await wait(page.locator('#start').click());
      await wait(page.waitForFunction(()=>window.labEvidence.accepted||window.labEvidence.error,null,{timeout:1200000}));
      const first=await wait(page.evaluate(()=>window.labEvidence));
      assertRuntime(first,engineName);assertPass(first,1);evidence.passes=first.passes;
      await wait(page.locator('#verify').click());
      await wait(page.waitForFunction(()=>window.labEvidence.checks===2||window.labEvidence.error,null,{timeout:180000}));
      const second=await wait(page.evaluate(()=>window.labEvidence));
      assertRuntime(second,engineName);assertPass(second,2,first.passes[0]);evidence.passes=second.passes;
      // The original result precedes the synchronous after-verification snapshot.
      // This bounded observation wait is still inside the same overall host budget.
      await wait(page.waitForFunction(()=>window.labEvidence.error||
        window.__browserClockDiagnostic.clockRows.some(row=>row.text.startsWith('[browser-clock] end id=1004 ')),
        null,{timeout:10000}));
      assertPass(await wait(page.evaluate(()=>window.labEvidence)),2,first.passes[0]);
      assert.deepEqual(evidence.pageErrors,[]);
      evidence.twoRealSequencesObserved=true;evidence.outcome='two_real_sequences_observed';
    } catch(error) {
      evidence.outcome=error===hostTimeout?'host_deadline':'runtime_or_preflight_failure';
      evidence.failure=String(error);
      if(!evidence.vmStartRequested) fatalCaptureFailure=true;
    } finally {
      clearTimeout(timer);evidence.stopRequestedAt=new Date().toISOString();
      evidence.observationElapsedMs=Math.round(performance.now()-started);
      try {
        let cutoff={error:'Page unavailable at capture cutoff'};
        if(page) {
          const captureDeadline=performance.now()+10000,remaining=()=>Math.max(0,captureDeadline-performance.now());
          cutoff=await drainGuestLogWithin(page.evaluate(()=>window.__freezeDiagnosticLog()),remaining())
            .catch(error=>({error:String(error)}));
          if(!cutoff.error) {
            const drained=await drainGuestLogWithin(page.evaluate(()=>window.__browserLabLogDrain),remaining())
              .catch(error=>({error:String(error)}));
            cutoff={...cutoff,...drained};
          }
        }
        evidence.guest=cutoff.guest??null;evidence.phase=cutoff.phase??null;evidence.detail=cutoff.detail??null;
        evidence.actualArguments=cutoff.arguments??null;evidence.diagnosticObserver=cutoff.diagnostic??null;
        const captured=log.snapshot(cutoff.expectedSequence);
        evidence.consoleTail=captured.text.slice(-4000);
        evidence.consoleTailTruncated=captured.text.length>4000;
        evidence.logCapture={...captured.metadata,...(cutoff.error?{error:cutoff.error}:{})};
        if(cutoff.error||cutoff.deliveredSequence!==cutoff.expectedSequence) evidence.logCapture.complete=false;
        const receiptJsonl=timeline.map(([firstSequence,sourceChunks,hostMonotonicMs])=>
          JSON.stringify({firstSequence,sourceChunks,hostMonotonicMs})+'\n').join('');
        await fs.writeFile(path.join(output,name+'-console.txt'),captured.text);
        await fs.writeFile(path.join(output,name+'-stream.jsonl'),captured.jsonl);
        await fs.writeFile(path.join(output,name+'-host-timeline.jsonl'),receiptJsonl);
        evidence.captureIntegrity={...verifyCapture(captured,timeline),hostTimelineSha256:digest(Buffer.from(receiptJsonl))};
        evidence.clocks=clockReport(cutoff.diagnostic,timeline,cutoff.expectedSequence);
        if(!evidence.logCapture.complete) {
          evidence.outcome='incomplete_capture';fatalCaptureFailure=true;
        }
        if(evidence.guest?.startupManifest) assert.deepEqual(evidence.guest.startupManifest,expectedStartup);
        if(evidence.guest?.vm?.qemuLiveInstanceLimit!=null) assertRuntime(evidence.guest,engineName);
        if(evidence.actualArguments) {
          const args=evidence.actualArguments;
          for(const flag of ['-accel','-smp','-m']) assert.equal(args.filter(x=>x===flag).length,1);
          assert.equal(args[args.indexOf('-accel')+1],'tcg,tb-size=500,thread=multi');
          assert.equal(args[args.indexOf('-smp')+1],'2,sockets=2,cores=1,threads=1');
          assert.equal(args.filter(x=>x==='-append').length,1);
          assert(!args[args.indexOf('-append')+1].split(/\s+/).includes('acpi=off'));
          assert(!args.includes('-no-acpi')&&!args.includes('-no-hpet'));
          assert.equal(args[args.indexOf('-m')+1],'512M');
          assert(!args.some(x=>['-machine','-M','-cpu','-icount','-rtc','-incoming'].includes(x)));
          if(expectedArgs===undefined) expectedArgs=args;else assert.deepEqual(args,expectedArgs);
        }
        if(evidence.twoRealSequencesObserved) {
          if(evidence.guest?.error||!evidence.guest?.ready||!evidence.guest?.accepted||evidence.pageErrors.length) {
            evidence.twoRealSequencesObserved=false;
            if(evidence.logCapture.complete) evidence.outcome='real_results_then_guest_or_page_failure';
          } else {
            assertPass(evidence.guest,2,evidence.passes[0]);
            assert.deepEqual(evidence.guest.passes,evidence.passes);
          }
          if(!evidence.logCapture.complete) evidence.twoRealSequencesObserved=false;
        }
      } catch(error) {
        fatalCaptureFailure=true;evidence.captureFailure=String(error);
        if(evidence.logCapture) evidence.logCapture.complete=false;
        evidence.outcome='harness_integrity_failure';evidence.twoRealSequencesObserved=false;
      } finally {
        if(browser) {
          try {await drainGuestLogWithin(browser.close(),10000);}
          catch(error) {evidence.cleanupFailure=String(error);cleanupFailed=true;}
        }
        if(evidence.twoRealSequencesObserved&&evidence.pageErrors.length) {
          evidence.twoRealSequencesObserved=false;evidence.outcome='real_results_then_page_failure';
        }
        server.closeAllConnections();
        try {await drainGuestLogWithin(new Promise(resolve=>server.close(resolve)),1000);}
        catch(error) {evidence.serverCleanupFailure=String(error);cleanupFailed=true;}
        evidence.finishedAt=new Date().toISOString();await write(name+'.json',evidence);
        const clockSamples=evidence.clocks?.samples??[],intervals=evidence.clocks?.intervals??[];
        const selectSample=sample=>({id:sample.id,point:sample.point,source:sample.source,time:sample.time,
          hostReceiptSpanMs:sample.hostReceiptSpanMs,available:sample.available,events:sample.events,
          intr:sample.intr,softirqs:sample.softirqs,irqStatus:sample.irqStatus});
        const compact={engine:engineName,browser:evidence.browser,outcome:evidence.outcome,
          runtimeSourceHead:prepared.runtimeSourceHead,hostSourceHead:prepared.hostSourceHead,kernelArtifact:prepared.kernelPin.artifact,
          vmStartRequested:evidence.vmStartRequested,observationElapsedMs:evidence.observationElapsedMs,
          phase:evidence.phase?.slice(0,512),detail:evidence.detail?.slice(0,1024),
          consoleTail:evidence.consoleTail,consoleTailTruncated:evidence.consoleTailTruncated,
          actualArguments:evidence.actualArguments,reportedLogicalCPUs:evidence.browserEnvironment?.reportedLogicalCPUs,
          guestBooted:evidence.guest?.booted??null,guestReady:evidence.guest?.ready??null,
          guestError:evidence.guest?.error??null,checks:evidence.guest?.checks??null,passes:(evidence.guest?.passes??[]).map(pass=>({id:pass.id,accepted:pass.accepted,statuses:pass.statuses})),
          twoRealSequencesObserved:evidence.twoRealSequencesObserved,captureComplete:evidence.logCapture?.complete===true,
          captureIntegrity:evidence.captureIntegrity,pageErrors:evidence.pageErrors.slice(0,8).map(value=>value.slice(0,1024)),
          failure:evidence.failure?.slice(0,2048),captureFailure:evidence.captureFailure?.slice(0,2048),
          cleanupFailure:evidence.cleanupFailure?.slice(0,1024),summaryTextBounded:true,
          clocks:{correctness:'not_established',sourceCounterInterpretation:evidence.clocks?.interruptInterpretation,framingComplete:evidence.clocks?.framingComplete??false,
            samples:clockSamples.length,issues:evidence.clocks?.issues?.slice(0,8),
            first:clockSamples.slice(0,2).map(selectSample),last:clockSamples.slice(-2).map(selectSample),
            longestReceiptIntervals:[...intervals].sort((a,b)=>b.hostReceiptDeltaMs-a.hostReceiptDeltaMs).slice(0,4),
            zeroUptimeDeltaIntervals:intervals.filter(x=>x.guestUptimeDeltaSeconds===0).slice(0,4)},
          services:{total:evidence.diagnosticObserver?.serviceCount??0,
            textTruncated:evidence.diagnosticObserver?.serviceSummaryTruncated??false,
            head:evidence.diagnosticObserver?.serviceHead?.slice(0,4)??[],
            tail:evidence.diagnosticObserver?.serviceTail?.slice(-12)??[]}};
        summary.attempts.push(compact);await write('summary.json',summary);
        console.log('DIAGNOSTIC_ONLY '+JSON.stringify(compact));
      }
    }
    if(fatalCaptureFailure) throw new Error('Stopped remaining engine after preflight/capture/provenance failure');
    if(cleanupFailed) throw new Error('Stopped remaining engine after browser/server cleanup failure');
  }
  summary.selectedEngineObserved=summary.attempts.length===1;
  summary.allCapturesComplete=summary.attempts.every(x=>x.captureComplete);
  await write('summary.json',summary);
  assert(summary.allCapturesComplete,'At least one diagnostic capture is incomplete');
} catch(error) {
  summary.harnessFailure=String(error);await write('summary.json',summary);process.exitCode=1;
}
