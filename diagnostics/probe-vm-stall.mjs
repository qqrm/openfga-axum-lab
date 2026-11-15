// One instrumented VM for diagnosis only. This script never claims acceptance.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createReadStream, promises as fs, appendFileSync, writeFileSync} from 'node:fs';
import {createServer} from 'node:http';
import {createRequire} from 'node:module';
import path from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';

const engineName = process.env.BROWSER_DIAGNOSTIC_ENGINE;
assert.ok(['chromium', 'firefox'].includes(engineName), 'Choose one browser engine');
const chromiumChannel = process.env.PROBE_CHROMIUM_CHANNEL ?? '';
assert.ok(chromiumChannel === '' || (engineName === 'chromium' && chromiumChannel === 'chrome'),
  'The only diagnostic browser override is system Chrome for Chromium');
const stopAfterHmpInput = process.env.PROBE_STOP_AFTER_HMP ?? 'false';
assert.ok(['true', 'false'].includes(stopAfterHmpInput), 'PROBE_STOP_AFTER_HMP must be true or false');
const stopAfterHmp = stopAfterHmpInput === 'true';
const source = path.resolve(process.env.PROBE_SOURCE_ROOT ?? 'source-candidate');
const root = path.join(source, 'browser-output');
const output = path.resolve(process.env.PROBE_OUTPUT_ROOT ?? `artifacts/pr36-host-probe/${engineName}`);
const budget = Number(process.env.PROBE_HOST_TIMEOUT_MS ?? 660000);
assert.ok(Number.isSafeInteger(budget) && budget >= 600000 && budget <= 720000, 'Host budget must be 10–12 minutes');
await fs.mkdir(output, {recursive: true});
const artifact = suffix => path.join(output, `${engineName}-${suffix}`);
const now = () => new Date().toISOString();
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function bounded(label, operation, timeout = 2000) {
  let timer;
  try {
    return await Promise.race([operation, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label}: host deadline ${timeout} ms`)), timeout);
    })]);
  } finally { clearTimeout(timer); }
}
async function hashFile(filename) {
  const hash = createHash('sha256');
  let bytes = 0;
  for await (const chunk of createReadStream(filename)) { hash.update(chunk); bytes += chunk.length; }
  return {bytes, sha256: hash.digest('hex')};
}
const result = {kind: 'instrumented_vm_stall_diagnostic', engine: engineName, startedAt: now(),
  acceptanceVerified: false, browserStarts: 0, pageErrors: [], observationErrors: [],
  requestedChromiumChannel: chromiumChannel || null, stopAfterHmp,
  configuredHostBudgetMs: budget, firstInterventionAt: null, hmpRounds: 0, completed: false};
let endAt = Date.now() + budget;
if (process.env.PROBE_JOB_TIMING_FILE) {
  const timing = JSON.parse(await fs.readFile(process.env.PROBE_JOB_TIMING_FILE, 'utf8'));
  const start = Date.parse(timing.started_at);
  assert.ok(Number.isFinite(start), 'Job timing must include the actual started_at');
  result.jobTiming = timing;
  endAt = Math.min(endAt, start + 27 * 60000);
}
assert.ok(endAt - Date.now() >= 60000, 'Not enough remaining job time for a bounded diagnostic');
result.hostDeadline = new Date(endAt).toISOString();
const hardStop = setTimeout(() => {
  result.failure = 'Host shutdown deadline exceeded';
  result.completed = false;
  result.finishedAt = now();
  writeFileSync(artifact('result.json'), JSON.stringify(result, null, 2));
  process.exit(2);
}, Math.max(1, endAt - Date.now() + 20000));
hardStop.unref();

const require = createRequire(path.join(source, 'e2e/package.json'));
const {chromium, firefox} = require('@playwright/test');
const {createGuestLogCollector} = await import(pathToFileURL(path.join(source, 'browser-single/pages/log-stream.mjs')));
let receiptAt;
const collector = createGuestLogCollector({now: () => receiptAt});
let lastChunkAt = Date.now(), sequence = 0, tail = '', sawBoot = false;
let browser, page, server, browserCdp;
const workers = new Map();
const timedOutWorkers = new Set();
let workerId = 0;
const append = (suffix, value) => appendFileSync(artifact(suffix), JSON.stringify(value) + '\n');
for (const name of ['console.txt', 'stream.jsonl', 'samples.jsonl', 'hmp.jsonl', 'browser-console.jsonl']) writeFileSync(artifact(name), '');

// Both changes are served only by this diagnostic HTTP server. Source files,
// WASM, kernel, native executables and the guest wrapper remain byte-identical.
const commands = ['\x01c', 'info status\n', 'info cpus\n', 'info registers -a\n', 'info irq\n', 'info pic\n'];
const launcherHook = `\n// PR36 diagnostic-only host input; no guest shell or state-changing HMP commands.\nwindow.__pr36DiagnosticSend = text => {\n  if (!${JSON.stringify(commands)}.includes(text)) throw new Error('Unreviewed diagnostic input');\n  input.push(...new TextEncoder().encode(text));\n  for (const callback of readable) callback();\n};\n`;
const loaderHook = `\n// PR36 diagnostic-only read of this worker's existing counters.\nglobalThis.__pr36QemuSnapshot = () => {\n  const t = Module.__wasm32_tb;\n  if (!t) return {initialized:false};\n  const v = new DataView(HEAP8.buffer);\n  return {initialized:true,core:t.cur_core_num,diagnostics:{...t.diagnostics},\n    tb:v.getUint32(t.tb_ptr_ptr,true),\n    gcCallbacksNotYetAccounted:v.getUint32(t.instance_garbage_collected_ptr,true),\n    removePending:v.getUint32(t.to_remove_instance_idx_ptr,true)};\n};\n`;
const overlay = new Map();
try {
  const commit = execFileSync('git', ['-C', source, 'rev-parse', 'HEAD'], {encoding: 'utf8'}).trim();
  assert.equal(commit, '444b069daf16e9606a0bbf1ea0aea7f4d61b97d8', 'Wrong runtime source');
  const wasmRoot = path.join(root, 'wasm');
  const manifest = JSON.parse(await fs.readFile(path.join(wasmRoot, 'runtime/startup-manifest.json'), 'utf8'));
  const derived = JSON.parse(await fs.readFile(path.join(wasmRoot, 'runtime/derived-runtime.json'), 'utf8'));
  const emulator = JSON.parse(await fs.readFile(path.join(wasmRoot, 'runtime/emulator-build/build.json'), 'utf8'));
  const files = ['launcher.js', 'guest.sh', 'entrypoint.sh', 'runtime/openfga', 'runtime/bootstrap',
    'runtime/out.js', 'runtime/qemu-system-x86_64.wasm', 'runtime/qemu-system-x86_64.data',
    'runtime/load.js', 'runtime/arg-module.js', 'runtime/kernel-build/bzImage',
    'runtime/startup-manifest.json', 'runtime/derived-runtime.json', 'runtime/emulator-build/build.json'];
  const identities = Object.fromEntries(await Promise.all(files.map(async name => [name, await hashFile(path.join(wasmRoot, name))])));
  for (const [name, expected] of Object.entries(manifest.files)) assert.deepEqual(identities[name], expected, `Startup identity mismatch: ${name}`);
  for (const [name, expected] of Object.entries(emulator.files)) assert.deepEqual(identities[`runtime/${name}`], expected, `Emulator identity mismatch: ${name}`);
  assert.deepEqual(identities['runtime/qemu-system-x86_64.data'], derived.data, 'Rootfs/kernel data identity mismatch');
  assert.equal(identities['runtime/load.js'].sha256, derived.loader_sha256);
  assert.equal(identities['runtime/kernel-build/bzImage'].sha256, derived.kernel_sha256);
  const launcher = await fs.readFile(path.join(wasmRoot, 'launcher.js'), 'utf8');
  assert.equal(launcher, await fs.readFile(path.join(source, 'browser-single/pages/launcher.js'), 'utf8'));
  assert.ok(launcher.includes("'2,sockets=2,cores=1,threads=1'"), 'Diagnostic must retain baseline two-vCPU topology');
  const loader = await fs.readFile(path.join(wasmRoot, 'runtime/out.js'), 'utf8');
  const anchor = 'var Module = moduleArg;';
  assert.equal(loader.split(anchor).length, 2, 'Loader insertion point must be unique');
  overlay.set('/wasm/launcher.js', Buffer.from(launcher + launcherHook));
  overlay.set('/wasm/runtime/out.js', Buffer.from(loader.replace(anchor, anchor + loaderHook)));
  result.provenance = {runtimeSourceCommit: commit, harness: await hashFile(fileURLToPath(import.meta.url)),
    originalFiles: identities, startupManifest: manifest, derivedRuntime: derived, servedOverlays: {},
    instrumentation: 'JS-only read-only worker observer and allowlisted serial monitor input; not pristine browser acceptance'};
  if (chromiumChannel === 'chrome') {
    assert.equal(process.platform, 'linux', 'System Chrome diagnostic is scoped to the Linux runner');
    const executable = '/opt/google/chrome/chrome';
    result.provenance.browserOverride = {channel:'chrome', executable,
      realPath:await fs.realpath(executable), ...await hashFile(executable),
      comparisonScope:'Runner system Chrome/new headless; not the pinned Playwright headless-shell engine'};
  }
  for (const [url, bytes] of overlay) {
    const name = path.basename(url);
    result.provenance.servedOverlays[url] = {bytes: bytes.length, sha256: digest(bytes)};
    await fs.writeFile(path.join(output, `${name}.served`), bytes);
  }
  await fs.writeFile(path.join(output, 'launcher-hook.txt'), launcherHook);
  await fs.writeFile(path.join(output, 'loader-hook.txt'), loaderHook);
  await fs.writeFile(artifact('provenance.json'), JSON.stringify(result.provenance, null, 2));

  const types = {'.html':'text/html', '.js':'text/javascript', '.mjs':'text/javascript', '.css':'text/css', '.wasm':'application/wasm', '.json':'application/json'};
  server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, 'http://localhost');
      const served = overlay.get(url.pathname);
      if (served) { response.writeHead(200, {'Content-Type':'text/javascript', 'Content-Length':served.length}); response.end(served); return; }
      const file = path.resolve(root, `.${decodeURIComponent(url.pathname)}${url.pathname.endsWith('/') ? 'index.html' : ''}`);
      if (!file.startsWith(root + path.sep)) throw new Error('Outside runtime root');
      const stat = await fs.stat(file);
      if (!stat.isFile()) throw new Error('Not a regular file');
      response.writeHead(200, {'Content-Type':types[path.extname(file)] ?? 'application/octet-stream', 'Content-Length':stat.size});
      const stream = createReadStream(file);
      stream.on('error', error => response.destroy(error));
      stream.pipe(response);
    } catch { if (!response.headersSent) response.writeHead(404); response.end(); }
  });
  await bounded('HTTP listen', new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); }), 5000);
  const engine = engineName === 'chromium' ? chromium : firefox;
  const launchOptions = {headless:true, timeout:30000, ...(engineName === 'chromium' ? {chromiumSandbox:true} : {}),
    ...(chromiumChannel ? {channel:chromiumChannel} : {})};
  result.launchOptions = launchOptions;
  result.browserStarts++;
  browser = await bounded('browser launch', engine.launch(launchOptions), 35000);
  result.browserVersion = browser.version();
  if (engineName === 'chromium') {
    try { browserCdp = await bounded('Chromium process observer', browser.newBrowserCDPSession(), 2000); }
    catch (error) { result.observationErrors.push({at:now(), kind:'process_observer', error:String(error)}); }
  }
  const context = await bounded('browser context', browser.newContext(), 5000);
  page = await bounded('browser page', context.newPage(), 5000);
  page.on('pageerror', error => result.pageErrors.push({at:now(), error:String(error)}));
  page.on('crash', () => result.pageErrors.push({at:now(), error:'page crashed'}));
  page.on('console', message => append('browser-console.jsonl', {at:now(), type:message.type(), text:message.text()}));
  page.on('worker', worker => {
    const info = {id:++workerId, url:worker.url(), createdAt:now(), closedAt:null};
    workers.set(worker, info);
    worker.on('close', () => { info.closedAt = now(); });
  });
  await page.exposeBinding('__recordPr36Log', (_source, batch) => {
    receiptAt = now();
    collector.appendBatch(batch);
    lastChunkAt = Date.now();
    sequence = batch.at(-1).sequence;
    const texts = batch.map(chunk => chunk.text);
    append('stream.jsonl', {firstSequence:batch[0].sequence, receivedAt:receiptAt, texts});
    appendFileSync(artifact('console.txt'), texts.join(''));
    tail = (tail + texts.join('')).slice(-24000);
    sawBoot ||= /LAB\|boot(?:\r?\n)/.test(tail);
  });
  await page.exposeBinding('__reportPr36LogFailure', (_source, message) => {
    result.observationErrors.push({at:now(), kind:'log_delivery', error:String(message)});
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/wasm/`, {timeout:30000});
  await page.waitForFunction(() => crossOriginIsolated && !document.getElementById('start').disabled, null, {timeout:30000});
  await bounded('install log subscription', page.evaluate(async () => {
    const {subscribeToGuestLog} = await import('./log-stream.mjs');
    window.__finishBrowserLabLog = subscribeToGuestLog(window,
      batch => window.__recordPr36Log(batch), {onError:error => window.__reportPr36LogFailure(String(error))});
    if (window.labLogSequence !== 0) throw new Error('Capture started after source output');
  }), 5000);
  result.vmStartRequestedAt = now();
  const vmStart = Date.now();
  await page.locator('#start').click({timeout:5000});
  let nextSample = 0, nextHmp = 0, lastHmpFinished = null, pageObservationTimedOut = false, hmpDisabled = false;
  while (Date.now() < endAt) {
    if (stopAfterHmp && result.hmpRounds === 3 && lastHmpFinished !== null && Date.now()-lastHmpFinished >= 30000) {
      result.stopReason = 'three_monitor_snapshots_collected';
      break;
    }
    if (Date.now() >= nextSample && !pageObservationTimedOut) {
      const sample = {at:now(), kind:'main_page_heartbeat', hostSequence:sequence, silentHostMs:Date.now()-lastChunkAt};
      try {
        sample.value = await bounded('main page observation', page.evaluate(() => ({wallTimeMs:Date.now(),
          performanceTimeMs:performance.now(), logSequence:window.labLogSequence,
          phase:document.getElementById('phase')?.textContent,
          guest:{ready:window.labEvidence?.ready, accepted:window.labEvidence?.accepted,
            checks:window.labEvidence?.checks, error:window.labEvidence?.error}, vm:window.labEvidence?.vm})), 2000);
        result.lastGuest = sample.value.guest;
      } catch (error) { sample.error = String(error); pageObservationTimedOut = true; }
      if (browserCdp) {
        try { sample.chromiumProcesses = await bounded('Chromium process CPU times', browserCdp.send('SystemInfo.getProcessInfo'), 2000); }
        catch (error) { sample.chromiumProcessError = String(error); browserCdp = undefined; }
      }
      append('samples.jsonl', sample);
      nextSample = Date.now() + 10000;
    }
    if (result.lastGuest?.error || result.pageErrors.length) { result.stopReason = 'observed_guest_or_page_error'; break; }
    if (!pageObservationTimedOut && !hmpDisabled && result.hmpRounds < 3 && Date.now() >= nextHmp && endAt-Date.now() > 30000 &&
        Date.now()-lastChunkAt >= 60000 && (sawBoot || Date.now()-vmStart >= 120000)) {
      result.firstInterventionAt ??= now();
      const round = {round:++result.hmpRounds, startedAt:now(), sourceSequenceBefore:sequence,
        silentHostMsBefore:Date.now()-lastChunkAt, workerObservations:[]};
      // These debugger evaluations can perturb worker scheduling/GC. They run
      // only after the recorded quiet baseline, never during ordinary startup.
      round.workerObservations = await Promise.all([...workers].map(async ([worker, info]) => {
        const observation = {...info, requestedAt:now()};
        if (info.closedAt || timedOutWorkers.has(worker)) return {...observation, skipped:'closed or prior evaluation still unresolved'};
        try {
          observation.value = await bounded(`worker ${info.id} observation`, worker.evaluate(() => ({wallTimeMs:Date.now(),
            snapshot:typeof globalThis.__pr36QemuSnapshot === 'function' ? globalThis.__pr36QemuSnapshot() : {observerUnavailable:true}})), 2000);
        } catch (error) { observation.error = String(error); timedOutWorkers.add(worker); }
        observation.receivedAt = now();
        return observation;
      }));
      append('samples.jsonl', {kind:'post_silence_workers', ...round});
      let switched = false;
      try {
        for (const command of commands) {
          append('hmp.jsonl', {round:round.round, at:now(), direction:'send', command, sequence});
          await bounded('diagnostic input', page.evaluate(text => window.__pr36DiagnosticSend(text), command), 2000);
          if (command === '\x01c') switched = true;
          await sleep(700);
        }
      } catch (error) {
        // A timed-out Playwright evaluation is not cancelled. Its queued bytes
        // may still arrive; do not guess monitor focus or toggle it next round.
        round.inputError = String(error);
        round.focusUnknown = result.hmpFocusUnknown = true;
        hmpDisabled = true;
      }
      finally {
        if (switched) {
          append('hmp.jsonl', {round:round.round, at:now(), direction:'send', command:'\x01c', purpose:'return_serial', sequence});
          await bounded('return serial focus', page.evaluate(() => window.__pr36DiagnosticSend('\x01c')), 2000)
            .catch(error => {
              round.returnSerialError = String(error);
              round.focusUnknown = result.hmpFocusUnknown = true;
              hmpDisabled = true;
            });
        }
      }
      await sleep(700);
      round.finishedAt = now();
      lastHmpFinished = Date.now();
      round.sourceSequenceAfter = sequence;
      round.consoleTailAfter = tail;
      append('hmp.jsonl', {direction:'observation', ...round});
      nextHmp = Date.now() + 60000;
    }
    await sleep(Math.min(500, Math.max(0, endAt-Date.now())));
  }
  result.stopReason ??= 'host_observation_budget_exhausted';
  result.completed = true;
} catch (error) {
  result.failure = String(error);
  process.exitCode = 1;
} finally {
  if (page) {
    try {
      const cutoff = await bounded('freeze source log', page.evaluate(async () => {
        const {freezeGuestLogCapture} = await import('./log-stream.mjs');
        return freezeGuestLogCapture(window);
      }), 3000);
      const drain = await bounded('drain source log', page.evaluate(() => window.__browserLabLogDrain), 3000);
      const captured = collector.snapshot(cutoff.expectedSequence);
      result.guest = cutoff.guest;
      result.logCapture = {...captured.metadata, deliveredSequence:drain.deliveredSequence,
        complete:captured.metadata.complete && !drain.error && drain.deliveredSequence === cutoff.expectedSequence};
      if (drain.error) result.logCapture.error = drain.error;
      await fs.writeFile(artifact('console.txt'), captured.text);
      // Rewrite from the canonical collector: its receipt timestamps were also
      // saved incrementally above, so interruption still leaves raw evidence.
      await fs.writeFile(artifact('stream.jsonl'), captured.jsonl);
    } catch (error) { result.logCapture = {complete:false, error:String(error), lastHostSequence:sequence}; }
    await bounded('screenshot', page.screenshot({path:artifact('screenshot.png'), fullPage:true, timeout:2000}), 2500)
      .catch(error => { result.screenshotError = String(error); });
  }
  if (!result.logCapture?.complete) process.exitCode = 1;
  result.workers = [...workers.values()];
  result.lastSourceReceiptAt = new Date(lastChunkAt).toISOString();
  result.finishedAt = now();
  await fs.writeFile(artifact('result.json'), JSON.stringify(result, null, 2));
  if (browser) await bounded('browser close', browser.close(), 5000).catch(error => {
    result.shutdownError = String(error); process.exitCode = 1;
  });
  if (server) {
    server.closeAllConnections();
    await bounded('HTTP close', new Promise(resolve => server.close(resolve)), 2000).catch(() => {});
  }
  await fs.writeFile(artifact('result.json'), JSON.stringify(result, null, 2));
  clearTimeout(hardStop);
  console.log(JSON.stringify({engine:engineName, kind:result.kind, completed:result.completed,
    acceptanceVerified:false, stopReason:result.stopReason, failure:result.failure,
    logCapture:result.logCapture, hmpRounds:result.hmpRounds, output}));
  // No pending Playwright evaluation may hold a CI worker after the evidence
  // has been saved and bounded shutdown attempted.
  process.exit(process.exitCode ?? 0);
}
