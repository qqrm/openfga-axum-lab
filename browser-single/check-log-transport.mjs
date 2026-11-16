// Real browser/host transport regression. No VM, service or authorization result.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';
import {readFile, mkdir, writeFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import path from 'node:path';
import {createGuestLogCollector, drainGuestLogWithin} from './pages/log-stream.mjs';

const require = createRequire(new URL('../e2e/package.json', import.meta.url));
const {chromium, firefox} = require('@playwright/test');
const source = await readFile(new URL('./pages/log-stream.mjs', import.meta.url));
const output = path.resolve('artifacts/browser-log-transport');
await mkdir(output, {recursive:true});
const server = createServer((request, response) => {
  if (request.url === '/log-stream.mjs') {
    response.writeHead(200, {'Content-Type':'text/javascript'});
    response.end(source);
    return;
  }
  response.writeHead(200, {'Content-Type':'text/html'});
  response.end(`<!doctype html><pre id="log"></pre><script type="module">
    import {createTranscriptLog, guestLogEvent} from './log-stream.mjs';
    window.labLogSequence = 0;
    window.writeLog = createTranscriptLog(text => {
      document.getElementById('log').textContent = text;
    }, chunk => {
      window.labLogSequence = chunk.sequence;
      window.dispatchEvent(new CustomEvent(guestLogEvent, {detail:chunk}));
    });
  </script>`);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const url = `http://127.0.0.1:${server.address().port}/`;
const failures = [];
// Each character is an original chunk, as observed from the serial UART.
// Finish with large identical tails and split UTF-16 to check exact retention.
const chunks = [...'UART:0123456789abcdefghijklmnopqrstuvwxyz\r\n'.repeat(750),
  'x'.repeat(150000), 'x'.repeat(150000), '\nUTF-8: ', '\ud83d', '\ude42', '\n'];
const expected = chunks.join('');
const digest = value => createHash('sha256').update(value).digest('hex');

try {
  for (const [name, engine] of [['chromium',chromium], ['firefox',firefox]]) {
    let browser;
    const collector = createGuestLogCollector();
    let calls = 0;
    let maximumBatch = 0;
    let cutoff;
    const evidence = {engine:name, passed:false, logModuleSha256:digest(source),
      scope:'Log transport only; no VM, services or authorization acceptance'};
    try {
      const executablePath = process.env[`BROWSER_LOG_${name.toUpperCase()}_EXECUTABLE`];
      browser = await engine.launch({headless:true, ...(executablePath ? {executablePath} : {})});
      evidence.browser = await browser.version();
      const page = await browser.newPage();
      const errors = [];
      page.on('pageerror', error => errors.push(String(error)));
      await page.goto(url);
      await page.waitForFunction(() => typeof window.writeLog === 'function');
      await page.exposeBinding('__recordLogBatch', async (_source, batch) => {
        calls++;
        maximumBatch = Math.max(maximumBatch, batch.length);
        collector.appendBatch(batch);
        // A slow receiver must not serialize one binding round trip per byte.
        await new Promise(resolve => setTimeout(resolve, 2));
      });
      await page.evaluate(async () => {
        const {subscribeToGuestLog} = await import('./log-stream.mjs');
        window.captureFailures = [];
        window.finishLog = subscribeToGuestLog(window, batch => window.__recordLogBatch(batch),
          {onError:error => window.captureFailures.push(String(error))});
      });
      await page.evaluate(chunks => {
        // One synchronous burst is intentionally harsher than interleaved UART.
        for (const chunk of chunks) window.writeLog(chunk);
      }, chunks);
      cutoff = await drainGuestLogWithin(page.evaluate(async () => {
        const expectedSequence = window.labLogSequence;
        const deliveredSequence = await window.finishLog();
        const before = window.labLogSequence;
        window.writeLog('after-cutoff');
        return {expectedSequence, deliveredSequence, afterSequence:window.labLogSequence,
          tailChars:document.getElementById('log').textContent.length,
          failures:window.captureFailures, before};
      }));
      assert.deepEqual(errors, []);
      assert.deepEqual(cutoff.failures, []);
      assert.equal(cutoff.expectedSequence, chunks.length);
      assert.equal(cutoff.deliveredSequence, chunks.length);
      assert.equal(cutoff.afterSequence, chunks.length + 1);
      assert.equal(cutoff.tailChars, 120000);
      const captured = collector.snapshot(cutoff.expectedSequence);
      assert.equal(captured.metadata.complete, true);
      assert.equal(captured.text, expected);
      assert(maximumBatch > 1 && maximumBatch <= 1024, 'Transport did not use bounded batches');
      assert(calls <= Math.ceil(chunks.length / 1024) + 1, 'Per-character binding regression');
      evidence.passed = true;
      evidence.bindingCalls = calls;
      evidence.maximumBatch = maximumBatch;
      evidence.transcriptSha256 = digest(captured.text);
      evidence.transcriptUtf8Bytes = Buffer.byteLength(captured.text);
      evidence.logCapture = captured.metadata;
      console.log(`${name}: batched log transport passed (${chunks.length} source chunks, ${calls} binding calls)`);
    } catch (error) {
      evidence.failure = String(error);
      failures.push(new Error(`${name}: ${error}`));
      console.error(`${name}: log transport failed: ${error}`);
    } finally {
      const captured = collector.snapshot(cutoff?.expectedSequence);
      evidence.logCapture ??= captured.metadata;
      await writeFile(path.join(output, `${name}.json`), JSON.stringify(evidence,null,2)+'\n');
      await writeFile(path.join(output, `${name}-stream.jsonl`), captured.jsonl);
      await writeFile(path.join(output, `${name}-console.txt`), captured.text);
      await browser?.close();
    }
  }
  if (failures.length) throw new AggregateError(failures, 'Actual browser log transport failed');
} finally {
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
