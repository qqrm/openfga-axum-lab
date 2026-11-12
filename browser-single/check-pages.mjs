// End-to-end browser VM check. This does not start Docker or a native backend.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createReadStream, promises as fs} from 'node:fs';
import path from 'node:path';
import {createRequire} from 'node:module';
import {expectedStatuses} from './pages/acceptance-protocol.mjs';
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
    if (failures.length) {
      await fs.writeFile(`artifacts/browser-wasm/${name}.json`,JSON.stringify({
        engine:name,passed:false,skipped:true,reason:'Earlier browser acceptance failed; stop before spending another VM boot budget'
      },null,2));
      console.log(`${name}: skipped because the earlier browser acceptance failed`);
      continue;
    }
    let browser;
    let page;
    const errors=[];
    const evidence={engine:name,passed:false,passes:[]};
    try {
      browser = await engine.launch({headless:true});
      evidence.browser = await browser.version();
      const context = await browser.newContext();
      page = await context.newPage();
      page.on('pageerror', error=>errors.push(String(error)));
      page.on('crash', ()=>errors.push('Browser page crashed'));
      await page.goto(url);
      await page.waitForFunction(()=>crossOriginIsolated && !document.getElementById('start').disabled, null, {timeout:30000});
      assert.equal(await page.evaluate(()=>window.labEvidence.accepted),false);
      assert.equal(await page.evaluate(()=>window.labEvidence.checks),0);
      await page.locator('#start').click();
      await page.waitForFunction(()=>window.labEvidence.accepted || window.labEvidence.error, null, {timeout:1200000});
      const first=await page.evaluate(()=>window.labEvidence);
      assert.equal(first.error,undefined,JSON.stringify(first));
      assert.ok(first.startupManifest,'Missing verified startup provenance');
      assert.equal(first.ready,true);
      assert.equal(first.accepted,true);
      assert.deepEqual(first.statuses,expectedStatuses);
      assert.equal(first.checks,1);
      assert.equal(first.passes.length,1);
      assert.equal(first.passes[0].id,1);
      assert.equal(first.passes[0].accepted,true);
      assert.deepEqual(first.passes[0].statuses,expectedStatuses);
      evidence.passes = first.passes;
      await page.locator('#verify').click();
      await page.waitForFunction(()=>window.labEvidence.checks===2 || window.labEvidence.error, null, {timeout:180000});
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
        evidence.guest = await page.evaluate(()=>window.labEvidence).catch(error=>({error:String(error)}));
        await fs.writeFile(`artifacts/browser-wasm/${name}-console.txt`,await page.locator('#log').textContent().catch(()=>errors.join('\n')));
        await page.screenshot({path:`artifacts/browser-wasm/${name}.png`,fullPage:true}).catch(error=>{evidence.screenshotError=String(error);});
      }
      if (evidence.passed && (evidence.guest?.error || !evidence.guest?.accepted || errors.length)) {
        evidence.passed = false;
        evidence.failure = 'Guest or browser failed after the second result';
        failures.push(new Error(`${name}: ${evidence.failure}`));
      }
      await fs.writeFile(`artifacts/browser-wasm/${name}.json`,JSON.stringify(evidence,null,2));
      await browser?.close();
    }
    if (evidence.passed) console.log(`${name}: real guest acceptance passed twice`);
  }
  if (failures.length) throw new AggregateError(failures,'Real browser acceptance did not pass twice in both engines');
} finally { server.closeAllConnections(); await new Promise(resolve=>server.close(resolve)); }
