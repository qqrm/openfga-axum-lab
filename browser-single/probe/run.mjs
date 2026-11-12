#!/usr/bin/env node
// Requires a local Playwright installation and a real Chromium executable.
import {createRequire} from 'node:module';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import fs from 'node:fs';
import path from 'node:path';

const require = createRequire(import.meta.url);
const args = process.argv.slice(2);
function option(name, fallback) {
  const index = args.indexOf(name);
  return index === -1 ? fallback : args[index + 1];
}
const directory = path.resolve(option('--out', 'probe-results'));
const timeout = Number(option('--timeout', '180')) * 1000;
const artifact = option('--artifact', null);
const chromiumPath = option('--chromium', process.env.CHROMIUM || '/usr/bin/chromium');
let url = option('--url', 'http://127.0.0.1:8765/');
fs.mkdirSync(directory, {recursive: true});
fs.writeFileSync(path.join(directory, 'events.jsonl'), '');
const events = [];
const record = (kind, value) => {
  const event = {time: new Date().toISOString(), kind, value};
  events.push(event);
  fs.appendFileSync(path.join(directory, 'events.jsonl'), JSON.stringify(event) + '\n');
};
let server = null;
let browser = null;
let interrupted = false;
process.on('SIGTERM', () => { interrupted = true; });
process.on('SIGINT', () => { interrupted = true; });
const started = Date.now();
let phase = 'setup';
let snapshot = null;
let serialLength = 0;
let terminalObservedAt = null;
let runError = null;
let guestFailure = null;
let hostRuntimeFailure = null;
let browserVersion = null;

async function startServer() {
  const serverScript = fileURLToPath(new URL('./serve.py', import.meta.url));
  const serverArgs = [serverScript, '--artifact', path.resolve(artifact), '--port', option('--port', '0')];
  const baseUrl = await new Promise((resolve, reject) => {
    let settled = false;
    let stdout = '';
    const timer = setTimeout(() => settle(new Error('Local static server did not become ready within 30 seconds.')), 30000);
    function settle(error, value) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) reject(error); else resolve(value);
    }
    server = spawn(option('--python', process.env.PYTHON || 'python3'), serverArgs, {stdio: ['ignore', 'pipe', 'pipe']});
    server.on('error', e => { record('server.error', e.message); settle(e); });
    server.on('exit', (code, signal) => {
      record('server.exit', {code, signal});
      settle(new Error(`Local static server exited before readiness (code=${code}, signal=${signal}).`));
    });
    server.stderr.on('data', bytes => record('server.stderr', bytes.toString()));
    server.stdout.on('data', bytes => {
      const text = bytes.toString();
      stdout += text;
      record('server.stdout', text);
      const match = stdout.match(/http:\/\/127\.0\.0\.1:\d+\//);
      if (match) settle(null, match[0]);
    });
  });
  const response = await fetch(baseUrl, {signal: AbortSignal.timeout(5000)});
  if (!response.ok) throw new Error(`Local static server readiness returned HTTP ${response.status}.`);
  await response.arrayBuffer();
  record('server.ready', {url: baseUrl, coop: response.headers.get('cross-origin-opener-policy'), coep: response.headers.get('cross-origin-embedder-policy')});
  if (response.headers.get('cross-origin-opener-policy') !== 'same-origin' || response.headers.get('cross-origin-embedder-policy') !== 'require-corp') {
    throw new Error('Local static server is missing the required COOP/COEP isolation headers.');
  }
  return baseUrl;
}

async function stopServer() {
  if (!server || server.exitCode !== null || server.signalCode !== null) return;
  await new Promise(resolve => {
    const timer = setTimeout(() => { server.kill('SIGKILL'); resolve(); }, 2000);
    server.once('exit', () => { clearTimeout(timer); resolve(); });
    server.kill('SIGTERM');
  });
}

try {
  if (!Number.isFinite(timeout) || timeout <= 0) throw new Error('--timeout must be a positive number of seconds.');
  if (artifact && args.includes('--url')) throw new Error('Use --artifact for an owned server or --url for an existing server, not both.');
  if (artifact) {
    phase = 'server-start';
    url = await startServer();
  }
  const query = option('--query', null);
  if (query) { const parsed = new URL(url); parsed.search = query; url = parsed.href; }
  phase = 'browser-launch';
  const {chromium} = require('playwright');
  browser = await chromium.launch({
    executablePath: chromiumPath,
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage', '--disable-background-timer-throttling'],
  });
  browserVersion = browser.version();
  const context = await browser.newContext();
  const page = await context.newPage();
  page.on('console', message => record('console.' + message.type(), message.text()));
  page.on('pageerror', e => {
    hostRuntimeFailure ??= e.stack || e.message;
    terminalObservedAt ??= Date.now();
    record('pageerror', e.stack || e.message);
  });
  page.on('crash', () => {
    hostRuntimeFailure ??= 'Browser page crashed.';
    terminalObservedAt ??= Date.now();
    record('crash', true);
  });
  page.on('requestfailed', request => record('requestfailed', {url: request.url(), failure: request.failure()}));
  page.on('websocket', socket => {
    record('websocket', socket.url());
    socket.on('socketerror', e => record('websocket.error', String(e)));
  });
  await context.route('**/*', route => {
    const requested = new URL(route.request().url());
    if (requested.origin !== new URL(url).origin) {
      record('blocked-external-request', requested.href);
      return route.abort('blockedbyclient');
    }
    return route.continue();
  });
  phase = 'page-navigation';
  await page.goto(url, {waitUntil: 'domcontentloaded', timeout: 60000});
  phase = 'guest-observation';
  while (!interrupted && Date.now() - started < timeout) {
    snapshot = await page.evaluate(() => globalThis.probe ? JSON.parse(JSON.stringify(globalThis.probe)) : null);
    if (snapshot && snapshot.serial.length !== serialLength) {
      serialLength = snapshot.serial.length;
      fs.writeFileSync(path.join(directory, 'serial.log'), snapshot.serial);
      console.log(JSON.stringify({elapsedSeconds: Math.round((Date.now()-started)/1000), status: snapshot.status, serialBytes: serialLength, tail: snapshot.serial.slice(-800)}));
    }
    // Unsupported syscall messages can belong to recoverable capability probes.
    // Preserve all serial output, but require an explicit terminal indication.
    const failure = snapshot?.serial.match(/Kernel panic[^\n]*|initdb: error[^\n]*|[^\n]*\bFATAL:[^\n]*/);
    if (failure) guestFailure ??= failure[0];
    if (snapshot?.failureOrigin === 'browser-runtime') {
      hostRuntimeFailure ??= snapshot.errors.at(-1)?.message || 'Browser/WASM runtime failed.';
    }
    if (snapshot?.status === 'exited' && snapshot.exitCode !== 0) guestFailure ??= `Runtime exited with code ${snapshot.exitCode}.`;
    if (guestFailure || (snapshot && ['aborted', 'failed', 'exited'].includes(snapshot.status))) terminalObservedAt ??= Date.now();
    if (terminalObservedAt && Date.now() - terminalObservedAt > 7000) break;
    await page.waitForTimeout(1000);
  }
} catch (e) {
  runError = e.stack || e.message;
  record('runner-error', {phase, error: runError});
} finally {
  if (browser) {
    try { await browser.close(); } catch (e) { record('browser.close-error', e.message); }
  }
  await stopServer();
  let outcome, exitCode;
  if (hostRuntimeFailure) { outcome = 'browser-runtime-failure'; exitCode = 2; }
  else if (runError) { outcome = 'infrastructure-failure'; exitCode = 2; }
  else if (interrupted) { outcome = 'interrupted'; exitCode = 130; }
  else if (guestFailure) { outcome = 'guest-failure'; exitCode = 3; }
  else if (snapshot && ['failed', 'aborted'].includes(snapshot.status)) { outcome = 'browser-runtime-failure'; exitCode = 2; }
  else if (snapshot?.status === 'exited') { outcome = 'runtime-exited'; exitCode = 0; }
  else { outcome = 'capture-timeout'; exitCode = 4; }
  const result = {
    url, artifact: artifact ? path.resolve(artifact) : null, chromiumPath,
    browser: browserVersion, elapsedMs: Date.now()-started,
    outcome, exitCode, phase, interrupted, runError, guestFailure, hostRuntimeFailure,
    authorizationVerified: false, httpBridgeImplemented: false,
    probe: snapshot, events,
  };
  fs.writeFileSync(path.join(directory, 'result.json'), JSON.stringify(result, null, 2));
  fs.writeFileSync(path.join(directory, 'serial.log'), snapshot?.serial || '');
  console.log(JSON.stringify({result: path.join(directory, 'result.json'), browser: result.browser, elapsedMs: result.elapsedMs, outcome, exitCode, phase, status: snapshot?.status, runError}));
  process.exitCode = exitCode;
}
