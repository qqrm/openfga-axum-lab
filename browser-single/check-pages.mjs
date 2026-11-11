// End-to-end browser VM check. This does not start Docker or a native backend.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createReadStream, promises as fs} from 'node:fs';
import path from 'node:path';
import {createRequire} from 'node:module';
const require = createRequire(new URL('../e2e/package.json', import.meta.url));
const {chromium, firefox} = require('@playwright/test');
const root = path.resolve('browser-output');
const types = {'.html':'text/html','.js':'text/javascript','.css':'text/css','.wasm':'application/wasm','.json':'application/json','.sh':'text/plain'};
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
try {
  for (const [name, engine] of [['chromium',chromium],['firefox',firefox]]) {
    const browser = await engine.launch({headless:true});
    const context = await browser.newContext();
    const page = await context.newPage();
    const errors=[];
    page.on('pageerror', error=>errors.push(String(error)));
    page.on('crash', ()=>errors.push('Browser page crashed'));
    try {
      await page.goto(url);
      await page.waitForFunction(()=>crossOriginIsolated && !document.getElementById('start').disabled, null, {timeout:30000});
      assert.equal(await page.evaluate(()=>window.labEvidence.accepted),false);
      await page.locator('#start').click();
      await page.waitForFunction(()=>window.labEvidence.accepted || window.labEvidence.error, null, {timeout:1200000});
      const first=await page.evaluate(()=>window.labEvidence);
      assert.equal(first.error,undefined,JSON.stringify(first));
      assert.equal(first.ready,true);
      assert.deepEqual(first.statuses,[403,204,200,204,403]);
      assert.equal(first.checks,1);
      await page.locator('#verify').click();
      await page.waitForFunction(()=>window.labEvidence.checks===2 || window.labEvidence.error, null, {timeout:180000});
      const second=await page.evaluate(()=>window.labEvidence);
      assert.equal(second.error,undefined,JSON.stringify(second));
      assert.equal(second.checks,2);
      assert.deepEqual(errors,[]);
      await fs.writeFile(`artifacts/browser-wasm/${name}.json`,JSON.stringify({browser:await browser.version(),...second},null,2));
      await page.screenshot({path:`artifacts/browser-wasm/${name}.png`,fullPage:true});
      console.log(`${name}: real guest acceptance passed twice`);
    } finally {
      await fs.writeFile(`artifacts/browser-wasm/${name}-console.txt`,await page.locator('#log').textContent().catch(()=>errors.join('\n')));
      await browser.close();
    }
  }
} finally { server.closeAllConnections(); await new Promise(resolve=>server.close(resolve)); }
