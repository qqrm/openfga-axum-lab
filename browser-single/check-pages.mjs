// End-to-end browser VM check. This does not start Docker or a native backend.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createReadStream, promises as fs} from 'node:fs';
import path from 'node:path';
import {createRequire} from 'node:module';
import {expectedStatuses} from './pages/acceptance-protocol.mjs';
import {createGuestLogCollector, drainGuestLogWithin} from './pages/log-stream.mjs';
const require = createRequire(new URL('../e2e/package.json', import.meta.url));
const {chromium, firefox} = require('@playwright/test');
const root = path.resolve('browser-output');
const types = {'.html':'text/html','.js':'text/javascript','.mjs':'text/javascript','.css':'text/css','.wasm':'application/wasm','.json':'application/json','.sh':'text/plain'};
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, 'http://localhost');
    const file = path.resolve(root, `.${decodeURIComponent(url.pathname)}${url.pathname.endsWith('/') ? 'index.html' : ''}`);
    if (!file.startsWith(root + path.sep)) throw new Error('Path outside fixture');
    const stat = await fs.stat(file);
    if (!stat.isFile()) throw new Error('Not a regular file');
    // No special server isolation headers: exercise the actual scoped worker.
    response.writeHead(200, {'Content-Type':types[path.extname(file)] || 'application/octet-stream','Content-Length':stat.size});
    createReadStream(file).pipe(response);
  } catch { response.writeHead(404); response.end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const url = `http://127.0.0.1:${server.address().port}/wasm/`;
await fs.mkdir('artifacts/browser-wasm', {recursive:true});
const failures=[];
try {
  for (const [name, engine] of [['chromium',chromium],['firefox',firefox]]) {
    let browser;
    let page;
    const guestLog = createGuestLogCollector();
    const errors=[];
    const evidence={engine:name,passed:false,passes:[],startedAt:new Date().toISOString()};
    console.log(`${name}: starting real guest acceptance`);
    try {
      browser = await engine.launch({headless:true});
      evidence.browser = await browser.version();
      const context = await browser.newContext();
      page = await context.newPage();
      let rejectPageFailure;
      const pageFailure = new Promise((_, reject) => { rejectPageFailure = reject; });
      // An error can arrive between waits. Record it without an unhandled
      // rejection, then let the same failure terminate each subsequent wait.
      pageFailure.catch(()=>{});
      const recordPageFailure = error => {
        errors.push(String(error));
        rejectPageFailure(error instanceof Error ? error : new Error(String(error)));
      };
      page.on('pageerror', recordPageFailure);
      page.on('crash', ()=>recordPageFailure('Browser page crashed'));
      const wait = operation => Promise.race([pageFailure,operation]);
      await wait(page.goto(url));
      await wait(page.waitForFunction(()=>crossOriginIsolated && !document.getElementById('start').disabled, null, {timeout:30000}));
      await page.exposeBinding('__recordBrowserLabLog', (_source, chunk) => {
        try { guestLog.append(chunk); }
        catch (error) { recordPageFailure(error); throw error; }
      });
      await page.exposeBinding('__reportBrowserLabLogFailure', (_source, message) => {
        recordPageFailure(new Error(message));
      });
      await page.evaluate(async () => {
        const {subscribeToGuestLog} = await import('./log-stream.mjs');
        window.__finishBrowserLabLog = subscribeToGuestLog(window,
          chunk => window.__recordBrowserLabLog(chunk),
          {onError: error => window.__reportBrowserLabLogFailure(String(error))});
      });
      assert.equal(await page.evaluate(()=>window.labLogSequence),0);
      assert.equal(await page.evaluate(()=>window.labEvidence.accepted),false);
      assert.equal(await page.evaluate(()=>window.labEvidence.checks),0);
      await wait(page.locator('#start').click());
      await wait(page.waitForFunction(()=>window.labEvidence.accepted || window.labEvidence.error, null, {timeout:1200000}));
      const first=await page.evaluate(()=>window.labEvidence);
      assert.equal(first.error,undefined,JSON.stringify(first));
      assert.ok(first.startupManifest,'Missing verified startup provenance');
      assert.equal(first.vm?.kernelLogLevel,7);
      assert.equal(first.startupManifest.openfga_build.warmup_cycles,1);
      assert.equal(first.startupManifest.openfga_build.model_validation_cache,'conditionless_exact_protobuf_body_except_id');
      assert.equal(first.startupManifest.openfga_build.model_validation_tests,'passed');
      assert.equal(first.startupManifest.openfga_build.tuple_write_typesystem_reuse,'same_request_store_and_model');
      assert.equal(first.startupManifest.openfga_build.write_typesystem_tests,'passed');
      assert.equal(first.startupManifest.emulator_build?.max_instance_alive_after,15000);
      assert.equal(first.startupManifest.emulator_build?.max_instance_running_local,2048);
      assert.equal(first.startupManifest.emulator_build?.instance_reservation,'atomic_compare_exchange');
      assert.equal(first.startupManifest.emulator_build?.runtime_diagnostics,'module_lifecycle_counters_v1');
      assert.equal(first.startupManifest.emulator_build?.runtime_diagnostics_tests,'passed');
      assert.equal(first.startupManifest.emulator_build?.i32_multiply_fix,'normalize_operands_before_widening_product');
      assert.equal(first.startupManifest.emulator_build?.multiply_emitter_preimage_blob,'3c85588be97b6c53a451e30c0593e8b433ede7b6');
      assert.equal(first.startupManifest.emulator_build?.multiply_emitter_postimage_blob,'d4078f5adac57d9ccc7d792773fff2ea65a33d6a');
      assert.equal(first.startupManifest.emulator_build?.i32_multiply_tests,'passed');
      assert.equal(first.ready,true);
      assert.equal(first.accepted,true);
      assert.deepEqual(first.statuses,expectedStatuses);
      assert.equal(first.checks,1);
      assert.equal(first.passes.length,1);
      assert.equal(first.passes[0].id,1);
      assert.equal(first.passes[0].accepted,true);
      assert.deepEqual(first.passes[0].statuses,expectedStatuses);
      evidence.passes = first.passes;
      await wait(page.locator('#verify').click());
      await wait(page.waitForFunction(()=>window.labEvidence.checks===2 || window.labEvidence.error, null, {timeout:180000}));
      const second=await page.evaluate(()=>window.labEvidence);
      assert.equal(second.error,undefined,JSON.stringify(second));
      assert.equal(second.checks,2);
      assert.equal(second.accepted,true);
      assert.deepEqual(second.statuses,expectedStatuses);
      assert.equal(second.passes.length,2);
      assert.deepEqual(second.passes[0],first.passes[0]);
      assert.equal(second.passes[1].id,2);
      assert.equal(second.passes[1].accepted,true);
      assert.deepEqual(second.passes[1].statuses,expectedStatuses);
      evidence.passes = second.passes;
      assert.deepEqual(errors,[]);
      evidence.passed = true;
    } catch (error) {
      evidence.failure = String(error);
      failures.push(new Error(`${name}: ${error}`));
      console.error(`${name}: real guest acceptance failed: ${error}`);
    } finally {
      evidence.pageErrors = errors;
      if (page) {
        // Freeze the subscription synchronously, then drain its pending host
        // deliveries. The UI tail is retained separately; it is not the source
        // of complete guest/service evidence.
        const captureEnd = await drainGuestLogWithin(page.evaluate(async () => {
          const expectedSequence = window.labLogSequence;
          const guest = structuredClone(window.labEvidence);
          if (!window.__finishBrowserLabLog) throw new Error('Guest log capture was not installed');
          const deliveredSequence = await window.__finishBrowserLabLog();
          return {expectedSequence, deliveredSequence, guest};
        })).catch(error=>({error:String(error)}));
        evidence.guest = captureEnd.guest ?? {error:captureEnd.error || 'Guest state unavailable at capture cutoff'};
        const captured = guestLog.snapshot(captureEnd.expectedSequence);
        evidence.logCapture = {...captured.metadata,
          ...(captureEnd.error ? {error:captureEnd.error} : {})};
        if (captureEnd.error || captureEnd.deliveredSequence !== captureEnd.expectedSequence) evidence.logCapture.complete = false;
        await fs.writeFile(`artifacts/browser-wasm/${name}-console.txt`,captured.text);
        await fs.writeFile(`artifacts/browser-wasm/${name}-stream.jsonl`,captured.jsonl);
        await fs.writeFile(`artifacts/browser-wasm/${name}-console-tail.txt`,await page.locator('#log').textContent({timeout:10000}).catch(()=>errors.join('\n')));
        await page.screenshot({path:`artifacts/browser-wasm/${name}.png`,fullPage:true,timeout:10000}).catch(error=>{evidence.screenshotError=String(error);});
      }
      if (evidence.passed && (evidence.guest?.error || !evidence.guest?.accepted || errors.length || !evidence.logCapture?.complete)) {
        evidence.passed = false;
        evidence.failure = 'Guest, browser or log capture failed after the second result';
        failures.push(new Error(`${name}: ${evidence.failure}`));
      }
      evidence.finishedAt = new Date().toISOString();
      await fs.writeFile(`artifacts/browser-wasm/${name}.json`,JSON.stringify(evidence,null,2));
      await browser?.close();
    }
    if (evidence.passed) console.log(`${name}: real guest acceptance passed twice`);
  }
  if (failures.length) throw new AggregateError(failures,'Real browser acceptance did not pass twice in both engines');
} finally { server.closeAllConnections(); await new Promise(resolve=>server.close(resolve)); }
