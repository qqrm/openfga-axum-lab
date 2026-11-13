// Real browser log transport proof only. No emulator, services or authorization responses.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import {spawnSync} from 'node:child_process';
import path from 'node:path';

const sourceCommit = '037a3c22a5fab45a735a6bfea9792c68049a4b62';
const sourceRoot = path.resolve('source-candidate');
const output = path.resolve('artifacts/browser-log-transport-candidate');
const revision = spawnSync('git',['rev-parse','HEAD'],{cwd:sourceRoot,encoding:'utf8'});
assert.equal(revision.status,0);
assert.equal(revision.stdout.trim(),sourceCommit);
assert.equal(spawnSync('git',['diff','--exit-code','HEAD','--'],{cwd:sourceRoot}).status,0);
const digest = value => createHash('sha256').update(value).digest('hex');
const inputPaths = ['browser-single/pages/log-stream.mjs','browser-single/pages/launcher.js',
  'browser-single/log-stream.test.mjs','browser-single/runtime-errors.test.mjs',
  'browser-single/acceptance-protocol.test.mjs','e2e/package.json','e2e/package-lock.json'];
const inputs = {};
for (const name of inputPaths) {
  const bytes = await readFile(path.join(sourceRoot,name));
  inputs[name] = {bytes:bytes.length,sha256:digest(bytes)};
}
const moduleBytes = await readFile(path.join(sourceRoot,inputPaths[0]));
const launcher = await readFile(path.join(sourceRoot,inputPaths[1]),'utf8');
assert.equal(launcher.split('window.__flushBrowserLabTranscript = transcriptRender.flush;').length,2);
const {createGuestLogCollector,drainGuestLogWithin} =
  await import(pathToFileURL(path.join(sourceRoot,inputPaths[0])).href);
const require = createRequire(path.join(sourceRoot,'e2e/package.json'));
const {chromium,firefox} = require('@playwright/test');
const fixtureSource = "import {createTranscriptLog, createCoalescedTranscriptRender, subscribeToGuestLog,\n  freezeGuestLogCapture, guestLogEvent} from '/log-stream.mjs';\n\nwindow.configureTransportCase = mode => {\n  const sentinel = new Error('cutoff render sentinel');\n  const stats = {renders:0, renderReports:0, sourceTasks:0, deliveryReports:[], sameRenderError:true};\n  window.labLogSequence = 0;\n  window.labEvidence = {transportOnly:true, transportFailed:false};\n  const renderer = createCoalescedTranscriptRender(text => {\n    stats.renders++;\n    if (mode === 'render_error') throw sentinel;\n    document.getElementById('log').textContent = text;\n  }, error => {\n    stats.renderReports++;\n    stats.sameRenderError &&= error === sentinel;\n    window.labEvidence.transportFailed = true;\n    window.labEvidence.transportError = String(error);\n  });\n  window.__flushBrowserLabTranscript = renderer.flush;\n  const write = createTranscriptLog(renderer.render, chunk => {\n    window.labLogSequence = chunk.sequence;\n    window.dispatchEvent(new CustomEvent(guestLogEvent,{detail:chunk}));\n  });\n  window.__finishBrowserLabLog = subscribeToGuestLog(window, batch => window.__recordLogBatch(batch), {\n    ...(mode === 'overflow' ? {maxPendingChunks:8} : {}),\n    onError:error => stats.deliveryReports.push(String(error))\n  });\n  window.emitChunks = chunks => { for (const chunk of chunks) write(chunk); };\n  window.emitTaskChunks = chunks => new Promise((resolve,reject) => {\n    const channel = new MessageChannel();\n    let index = 0;\n    channel.port1.onmessage = () => {\n      try {\n        stats.sourceTasks++;\n        write(chunks[index++]);\n        if (index < chunks.length) channel.port2.postMessage(null);\n        else { channel.port1.close(); channel.port2.close(); resolve(); }\n      } catch (error) { channel.port1.close(); channel.port2.close(); reject(error); }\n    };\n    if (chunks.length) channel.port2.postMessage(null);\n    else { channel.port1.close(); channel.port2.close(); resolve(); }\n  });\n  window.beginCapture = () => {\n    window.transportCutoff = freezeGuestLogCapture(window);\n    window.visibleAtCutoff = document.getElementById('log').textContent;\n    return {cutoff:window.transportCutoff,visible:window.visibleAtCutoff,stats:{...stats}};\n  };\n  window.burstAndFreeze = chunks => { window.emitChunks(chunks); return window.beginCapture(); };\n  window.readCapture = async () => {\n    const drain = await window.__browserLabLogDrain;\n    let sameStickyError = null;\n    if (mode === 'render_error') {\n      try { renderer.flush(); sameStickyError = false; }\n      catch (error) { sameStickyError = error === sentinel; }\n    }\n    const sourceBeforeLate = window.labLogSequence;\n    write('after cutoff');\n    try { renderer.flush(); } catch (error) {\n      if (mode !== 'render_error' || error !== sentinel) throw error;\n    }\n    return {cutoff:window.transportCutoff,drain,visible:window.visibleAtCutoff,\n      stats:{...stats},sameStickyError,sourceBeforeLate,sourceAfterLate:window.labLogSequence};\n  };\n};\nwindow.transportFixtureReady = true;\n";
const requests = [];
const server = createServer((request,response)=>{
  requests.push(request.url);
  if (requests.length>128) { response.writeHead(429);response.end();return; }
  const resources = {
    '/': ['text/html','<!doctype html><meta charset="utf-8"><pre id="log"></pre><script type="module" src="/fixture.mjs"></script>'],
    '/fixture.mjs': ['text/javascript',fixtureSource],
    '/log-stream.mjs': ['text/javascript',moduleBytes],
    '/favicon.ico': ['image/x-icon','']
  };
  const item=resources[request.url];
  response.writeHead(item?200:404,{'Content-Type':item?.[0]??'text/plain',
    'Cross-Origin-Opener-Policy':'same-origin','Cross-Origin-Embedder-Policy':'require-corp'});
  response.end(item?.[1]??'Not found');
});
await mkdir(output,{recursive:true});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const url='http://127.0.0.1:'+server.address().port+'/';
const within = (operation,ms=10000)=>drainGuestLogWithin(operation,ms);
const failures=[],results=[];
const scope='Transport only: real browser event tasks, binding and DOM; no VM, runtime downloads, services or authorization acceptance.';
const largeChunks=[...'UART:0123456789abcdefghijklmnopqrstuvwxyz\r\n'.repeat(750),
  'x'.repeat(150000),'x'.repeat(150000),'\nUTF-8: ','\ud83d','\ude42','\n'];
const taskChunks=[...Array.from({length:3077},(_,i)=>String.fromCharCode(65+i%26)),
  '', '\ud83d','\ude42','€','\n'];
const scenarios=[
  {name:'synchronous_uart_burst',chunks:largeChunks},
  {name:'separate_tasks_ack_gate',chunks:taskChunks,ackGate:true,tasks:true},
  {name:'separate_tasks_unblocked',chunks:taskChunks.slice(0,1307).concat(['','\ud83d','\ude42']),tasks:true},
  {name:'finish_partial_utf16',chunks:['partial:','','\ud83d','\ude42','€','\n']},
  {name:'render_error_at_cutoff',chunks:Array(1024).fill('r'),mode:'render_error'},
  {name:'delivery_error',chunks:Array(2050).fill('d'),mode:'delivery_error'},
  {name:'pending_chunk_bound',chunks:Array(9).fill('q'),mode:'overflow'}
];

function verifyJSONL(captured,expectedChunks) {
  const rows=captured.jsonl.split('\n').filter(Boolean).map(line=>JSON.parse(line));
  const actual=[];
  let sequence=1;
  for (const row of rows) {
    assert.equal(row.firstSequence,sequence);
    assert.match(row.receivedAt,/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
    assert(row.texts.length>0&&row.texts.length<=1024);
    for (const text of row.texts) {actual.push(text);sequence++;}
  }
  assert.deepEqual(actual,expectedChunks,'Original chunks changed order or UTF-16 boundaries');
  assert.equal(captured.text,expectedChunks.join(''));
  assert.equal(captured.metadata.bytes,expectedChunks.reduce((n,text)=>n+Buffer.byteLength(text),0));
  assert.equal(captured.metadata.chunks,expectedChunks.length);
}

const watchdog=setTimeout(()=>{console.error('Transport-only overall watchdog expired');process.exit(1);},150000);
try {
  for (const [engineName,engine] of [['chromium',chromium],['firefox',firefox]]) {
    let browser;
    try {
      browser=await within(engine.launch({headless:true}),20000);
      const browserVersion=await browser.version();
      assert.match(browserVersion,engineName==='firefox'?/^159\.0$/:/^158\./);
      for (const scenario of scenarios) {
        const started=performance.now();
        const collector=createGuestLogCollector();
        const page=await browser.newPage();
        page.setDefaultTimeout(10000);
        const pageErrors=[];
        page.on('pageerror',error=>pageErrors.push(String(error)));
        let calls=0,inFlight=0,maximumInFlight=0,maximumBatch=0;
        let releaseFirst,noticeFirst;
        const firstGate=new Promise(resolve=>{releaseFirst=resolve;});
        const firstSeen=new Promise(resolve=>{noticeFirst=resolve;});
        let before,after,captured;
        const proof={engine:engineName,browser:browserVersion,scenario:scenario.name,scope,
          passed:false,expectedFailure:Boolean(scenario.mode),sourceChunks:scenario.chunks.length};
        try {
          await page.exposeBinding('__recordLogBatch',async(_source,batch)=>{
            calls++;inFlight++;maximumInFlight=Math.max(maximumInFlight,inFlight);
            maximumBatch=Math.max(maximumBatch,batch.length);
            try {
              if(scenario.mode==='delivery_error')throw new Error('host delivery sentinel');
              collector.appendBatch(batch);
              if(scenario.ackGate&&calls===1){noticeFirst();await firstGate;}
            }finally{inFlight--;}
          });
          await page.goto(url,{timeout:10000});
          await page.waitForFunction(()=>window.transportFixtureReady===true);
          await within(page.evaluate(mode=>window.configureTransportCase(mode),scenario.mode??'normal'));
          if(scenario.ackGate){
            await within(page.evaluate(chunks=>window.emitChunks(chunks),scenario.chunks.slice(0,1024)));
            await within(firstSeen);
            await within(page.evaluate(chunks=>window.emitTaskChunks(chunks),scenario.chunks.slice(1024)));
            before=await within(page.evaluate(()=>window.beginCapture()));
            assert.equal(calls,1,'A second binding overlapped the unacknowledged first binding');
            releaseFirst();
          }else if(scenario.tasks){
            await within(page.evaluate(chunks=>window.emitTaskChunks(chunks),scenario.chunks));
            before=await within(page.evaluate(()=>window.beginCapture()));
          }else{
            before=await within(page.evaluate(chunks=>window.burstAndFreeze(chunks),scenario.chunks));
          }
          after=await within(page.evaluate(()=>window.readCapture()));
          captured=collector.snapshot(before.cutoff.expectedSequence);
          assert.deepEqual(pageErrors,[]);
          assert.equal(before.cutoff.expectedSequence,scenario.chunks.length);
          assert.equal(after.sourceBeforeLate,scenario.chunks.length);
          assert.equal(after.sourceAfterLate,scenario.chunks.length+1);
          assert.equal(maximumInFlight,1,'More than one host delivery was in flight');
          assert(maximumBatch>0&&maximumBatch<=1024);
          const expectedCaptured=scenario.mode==='delivery_error'?[]:
            scenario.mode==='overflow'?scenario.chunks.slice(0,8):scenario.chunks;
          verifyJSONL(captured,expectedCaptured);
          if(scenario.mode==='render_error'){
            assert.equal(before.stats.renders,1);
            assert.equal(before.stats.renderReports,1);
            assert.equal(before.cutoff.guest.transportFailed,true,'Render failure missed the synchronous cutoff');
            assert.equal(before.cutoff.guest.transportError,'Error: cutoff render sentinel');
            assert.equal(after.drain.error,'Error: cutoff render sentinel');
            assert.equal(after.sameStickyError,true);
            assert.equal(after.stats.sameRenderError,true);
            assert.equal(after.stats.renders,1,'A failed renderer was retried');
            assert.equal(after.stats.renderReports,1);
            assert.equal(captured.metadata.complete,true,'The renderer error must be tested even with full delivery');
            assert.equal(calls,1,'The cutoff failure case must have exactly one full batch');
          }else if(scenario.mode){
            assert.match(after.drain.error,scenario.mode==='overflow'?/capture limit/:/host delivery sentinel/);
            assert.equal(after.stats.deliveryReports.length,1);
            assert.equal(captured.metadata.complete,false);
            assert.equal(calls,1,'Delivery continued past a failed batch or bounded prefix');
          }else{
            assert.equal(after.drain.error,undefined);
            assert.equal(after.drain.deliveredSequence,scenario.chunks.length);
            assert.equal(before.cutoff.guest.transportFailed,false);
            assert.equal(captured.metadata.complete,true);
            assert.equal(before.visible,scenario.chunks.join('').slice(-120000));
            assert.deepEqual(after.stats.deliveryReports,[]);
            if(!scenario.tasks){
              assert.equal(before.stats.renders,1,'A synchronous source burst should have one visible cutoff render');
              assert.equal(calls,Math.ceil(scenario.chunks.length/1024));
            }
            if(scenario.ackGate)assert.equal(calls,Math.ceil(scenario.chunks.length/1024));
          }
          if(scenario.tasks)assert.equal(after.stats.sourceTasks,
            scenario.chunks.length-(scenario.ackGate?1024:0));
          proof.passed=true;
          proof.captureQualified=after.drain.error===undefined&&captured.metadata.complete&&
            before.cutoff.guest.transportFailed===false;
          assert.equal(proof.captureQualified,!scenario.mode);
        }catch(error){
          proof.failure=String(error);
          failures.push(new Error(engineName+'/'+scenario.name+': '+error));
        }finally{
          releaseFirst();
          captured??=collector.snapshot(before?.cutoff.expectedSequence);
          Object.assign(proof,{durationMs:Math.round((performance.now()-started)*1000)/1000,
            bindingCalls:calls,maximumInFlight,maximumBatch,
            before:before?{...before,visible:undefined,visibleChars:before.visible.length,visibleSha256:digest(before.visible)}:null,
            after:after?{...after,visible:undefined,visibleChars:after.visible.length,visibleSha256:digest(after.visible)}:null,pageErrors,
            logCapture:captured.metadata,consoleSha256:digest(captured.text),
            consoleUtf8Bytes:Buffer.byteLength(captured.text),streamSha256:digest(captured.jsonl)});
          const prefix=path.join(output,engineName+'-'+scenario.name);
          await writeFile(prefix+'-console.txt',captured.text);
          await writeFile(prefix+'-stream.jsonl',captured.jsonl);
          try{await within(page.close(),5000);}catch(error){proof.cleanupError=String(error);proof.passed=false;failures.push(error);}
          await writeFile(prefix+'.json',JSON.stringify(proof,null,2)+'\n');
          results.push(proof);
          console.log('TRANSPORT_ONLY '+JSON.stringify({...proof,failure:proof.failure?.slice(0,4000)}));
        }
      }
    }catch(error){failures.push(error);console.error(engineName+': transport-only browser failure: '+error);}
    finally{if(browser)try{await within(browser.close(),5000);}catch(error){failures.push(error);}}
  }
  assert(requests.every(url=>['/','/fixture.mjs','/log-stream.mjs','/favicon.ico'].includes(url)),
    'Fixture attempted an unexpected resource download');
  assert.equal(results.length,scenarios.length*2);
  assert.equal(failures.length,0,'Transport-only proof failed');
}catch(error){
  failures.push(error);
}finally{
  clearTimeout(watchdog);
  server.closeAllConnections();
  await new Promise(resolve=>server.close(resolve));
  const summary={schemaVersion:1,scope,sourceCommit,inputs,
    harnessSha256:digest(await readFile(new URL(import.meta.url))),fixtureSha256:digest(fixtureSource),
    nodeVersion:process.version,productionAcceptance:false,vmStarts:0,runtimeDownloads:0,
    unexpectedRequests:requests.filter(url=>!['/','/fixture.mjs','/log-stream.mjs','/favicon.ico'].includes(url)),
    expectedCases:scenarios.length*2,completedCases:results.length,
    passed:failures.length===0&&results.length===scenarios.length*2,
    cases:results.map(p=>({engine:p.engine,scenario:p.scenario,passed:p.passed,
      expectedFailure:p.expectedFailure,captureQualified:p.captureQualified,
      bindingCalls:p.bindingCalls,maximumInFlight:p.maximumInFlight,maximumBatch:p.maximumBatch,durationMs:p.durationMs})),
    requests,failures:failures.map(String)};
  await writeFile(path.join(output,'summary.json'),JSON.stringify(summary,null,2)+'\n');
  console.log('TRANSPORT_ONLY_SUMMARY '+JSON.stringify(summary));
}
if(failures.length)throw new AggregateError(failures,'Transport-only proof failed');
