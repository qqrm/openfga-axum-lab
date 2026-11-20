// Click the real page against one fresh browser-hosted Linux VM. No host API
// serves authorization decisions; the HTTP server below serves static files only.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createReadStream, promises as fs} from 'node:fs';
import {createServer} from 'node:http';
import {createRequire} from 'node:module';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {modelSha256, validateStartupManifest} from './pages/startup-payload.mjs';

const engineName = process.env.BROWSER_CONCEPT_ENGINE;
const site = path.resolve(process.env.BROWSER_CONCEPT_ROOT ?? 'browser-output/concept');
const output = path.resolve('artifacts/browser-concept-browser', engineName ?? 'unknown');
const sourceCommit = execFileSync('git', ['rev-parse', 'HEAD'], {encoding: 'utf8'}).trim();
const runId = Number(process.env.GITHUB_RUN_ID);
const expectedStatuses = [403, 204, 200, 204, 403];
const readJSON = async file => JSON.parse(await fs.readFile(file, 'utf8'));
async function record(file) {
  const stat = await fs.lstat(file);
  assert(stat.isFile(), 'Expected a regular file: ' + file);
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(file)) hash.update(bytes);
  return {bytes: stat.size, sha256: hash.digest('hex')};
}
async function inventory(directory, prefix = '') {
  const result = {};
  for (const item of await fs.readdir(directory, {withFileTypes: true})) {
    const name = prefix + item.name;
    assert(!item.isSymbolicLink(), 'Site symlink is not allowed: ' + name);
    if (item.isDirectory()) Object.assign(result, await inventory(path.join(directory, item.name), name + '/'));
    else result[name] = await record(path.join(directory, item.name));
  }
  return result;
}

await fs.mkdir(output, {recursive: true});
const result = {schemaVersion: 1, kind: 'real_browser_concept_check', scenario: 'document-permissions',
  engine: engineName, sourceCommit, runId, passed: false, vmStarted: false,
  lakekeeperAcceptance: false, productionAcceptance: false, startedAt: new Date().toISOString(),
  cycles: [], negativeControls: [], pageErrors: [], requests: [], browserConsole: []};
let browser, page, context, server, guestLog, drainGuestLogWithin, timer;
let checksPassed = false;
const save = () => fs.writeFile(path.join(output, 'result.json'), JSON.stringify(result, null, 2) + '\n');

try {
  assert(['chromium', 'firefox'].includes(engineName), 'An explicit supported browser is required');
  assert.equal(process.env.GITHUB_SHA, sourceCommit);
  assert.equal(process.env.GITHUB_RUN_ATTEMPT, '1');
  assert(Number.isSafeInteger(runId) && runId > 0);
  execFileSync('git', ['diff', '--exit-code', 'HEAD', '--'], {stdio: 'pipe'});

  const timing = await readJSON('prepared/job-timing.json');
  assert.equal(timing.source, 'GitHub Actions current job API');
  assert.equal(timing.name, `Browser concept (${engineName})`);
  assert.equal(timing.head_sha, sourceCommit);
  assert.equal(timing.run_id, runId);
  assert.equal(timing.run_attempt, 1);
  assert(Number.isSafeInteger(timing.id) && timing.id > 0);
  assert(Number.isFinite(Date.parse(timing.started_at)));
  result.jobTiming = timing;

  const assembly = await readJSON(path.join(site, 'provenance/assembly.json'));
  assert.equal(assembly.kind, 'verified_browser_concept_assembly');
  assert.equal(assembly.conceptSourceCommit, sourceCommit);
  assert.equal(assembly.producerRunId, runId);
  assert.equal(assembly.producerRunAttempt, 1);
  assert.equal(assembly.retainedRuntimeSource, '444b069daf16e9606a0bbf1ea0aea7f4d61b97d8');
  assert.equal(assembly.originalRootfsSource, '99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58');
  assert.equal(assembly.emulatorSource, '6f35f24b7ca345cf47c873becee6a41ef94e65a9');
  for (const key of ['rootfsUnchanged', 'kernelUnchanged', 'emulatorUnchanged', 'nativeIntegrationPassed',
    'all17RetainedOpenFGAInputsUnchanged']) assert.equal(assembly[key], true, key);
  assert.deepEqual(assembly.intendedConfiguration, {virtualCPUs: 2, memoryMiB: 512, postgresApplication: true,
    openfgaDatastore: 'memory', application: 'independent_two_type_document_concept', lakekeeperApplicationStarted: false});
  assert.deepEqual(assembly.changedRetainedRuntimeFiles, ['startup-manifest.json', 'derived-runtime.json']);
  assert.deepEqual(assembly.addedRuntimeFiles, ['concept']);
  const siteFiles = await inventory(site);
  result.assembly = siteFiles['provenance/assembly.json'];
  // assembly.json is the sole intentional self-reference exclusion.
  delete siteFiles['provenance/assembly.json'];
  assert.deepEqual(siteFiles, assembly.siteFiles, 'Assembled site bytes changed');
  for (const [name, expected] of Object.entries(assembly.originalRuntime)) {
    if (!assembly.changedRetainedRuntimeFiles.includes(name)) {
      assert.deepEqual(siteFiles['runtime/' + name], expected, 'Retained runtime drift: ' + name);
    }
  }
  for (const name of ['index.html', 'site.css', 'launcher.js', 'protocol.mjs', 'startup-payload.mjs', 'isolation-sw.js']) {
    assert.deepEqual(siteFiles[name], await record(path.join('browser-concept/pages', name)));
  }
  for (const name of ['guest.sh', 'entrypoint.sh', 'model.json']) {
    assert.deepEqual(siteFiles[name], await record(path.join('browser-concept', name)));
  }
  const manifest = validateStartupManifest(await readJSON(path.join(site, 'runtime/startup-manifest.json')));
  assert.equal(manifest.conceptSourceCommit, sourceCommit);
  assert.deepEqual(manifest.files['runtime/concept'], assembly.conceptBinary);
  assert.deepEqual(manifest.model, assembly.conceptModel);
  assert.deepEqual(manifest.nativeProof.record, siteFiles['provenance/native-proof.json']);
  assert.deepEqual(assembly.nativeIntegrationProof, manifest.nativeProof.record);
  const nativeProof = await readJSON(path.join(site, 'provenance/native-proof.json'));
  assert.equal(nativeProof.passed, true);
  assert.equal(nativeProof.sourceCommit, sourceCommit);
  assert.equal(nativeProof.runId, runId);
  assert.deepEqual(nativeProof.conceptBinary, assembly.conceptBinary);
  assert.deepEqual(nativeProof.model, manifest.model);
  assert.equal(nativeProof.bridge?.passed, true);
  assert.deepEqual(nativeProof.cycles.map(cycle => cycle.statuses), [expectedStatuses, expectedStatuses]);
  assert.equal(nativeProof.upstreamFailureDenied, true);
  result.inputProof = {conceptBinary: assembly.conceptBinary, model: assembly.conceptModel,
    nativeProof: assembly.nativeIntegrationProof, siteFilesVerified: Object.keys(siteFiles).length};
  if (engineName === 'firefox') {
    const transfer = await readJSON('prepared/concept-chromium-site-archive-provenance.json');
    assert.equal(transfer.kind, 'concept-chromium-site');
    assert.equal(transfer.producer.runId, runId);
    assert.equal(transfer.producer.workflowCommit, sourceCommit);
    assert.equal(transfer.producer.job.name, 'Browser concept (chromium)');
    assert.equal(transfer.producer.job.conclusion, 'success');
    assert.equal(transfer.consumerJob.id, timing.id);
    assert.deepEqual(transfer.members, {...siteFiles, 'provenance/assembly.json': result.assembly});
    result.chromiumBundle = {artifact: transfer.artifact, archive: transfer.archive,
      producerJobId: transfer.producer.job.id, consumerJobId: timing.id};
  }

  const helpers = await import(pathToFileURL(path.join(site, 'log-stream.mjs')).href);
  guestLog = helpers.createGuestLogCollector();
  drainGuestLogWithin = helpers.drainGuestLogWithin;
  const require = createRequire(path.resolve('e2e/package.json'));
  const {chromium, firefox} = require('@playwright/test');
  let launchOptions = {headless: true};
  let smoke;
  if (engineName === 'chromium') {
    smoke = await readJSON('artifacts/pr36-vcpu-gate/chromium/system-chrome-smoke.json');
    assert.equal(smoke.kind, 'system_chrome_sandbox_smoke');
    assert.equal(smoke.passed, true);
    assert.notEqual(process.getuid(), 0);
    assert.equal(smoke.uid, process.getuid());
    assert.equal(smoke.executable.path, '/opt/google/chrome/chrome');
    assert.equal(smoke.executable.realPath, await fs.realpath(smoke.executable.path));
    assert.deepEqual(await record(smoke.executable.path), {bytes: smoke.executable.bytes, sha256: smoke.executable.sha256});
    launchOptions = {channel: 'chrome', chromiumSandbox: true, headless: true, timeout: 30000,
      args: ['--enable-automation']};
    assert.deepEqual(smoke.launchOptions, launchOptions);
  }
  const staticRoot = path.dirname(site);
  const types = {'.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript',
    '.css': 'text/css', '.wasm': 'application/wasm', '.json': 'application/json', '.sh': 'text/plain'};
  server = createServer(async (request, response) => {
    try {
      assert(['GET', 'HEAD'].includes(request.method));
      const url = new URL(request.url, 'http://localhost');
      const file = path.resolve(staticRoot, '.' + decodeURIComponent(url.pathname) + (url.pathname.endsWith('/') ? 'index.html' : ''));
      assert(file.startsWith(site + path.sep), 'Only the static concept site is served');
      const stat = await fs.stat(file);
      assert(stat.isFile());
      // Exercise the scoped isolation worker, without special server headers.
      response.writeHead(200, {'Content-Type': types[path.extname(file)] ?? 'application/octet-stream',
        'Content-Length': stat.size});
      if (request.method === 'HEAD') response.end();
      else createReadStream(file).on('error', error => response.destroy(error)).pipe(response);
    } catch { response.writeHead(404); response.end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const url = origin + '/' + path.basename(site) + '/';
  result.launchOptions = launchOptions;
  browser = await (engineName === 'chromium' ? chromium : firefox).launch(launchOptions);
  result.browserVersion = browser.version();
  if (smoke) {
    assert.equal(result.browserVersion, smoke.actualVersion);
    const session = await browser.newBrowserCDPSession();
    const command = await session.send('Browser.getBrowserCommandLine');
    assert(!command.arguments.some(arg => /^--(?:no-sandbox|disable-(?:setuid|namespace|seccomp-filter)-sandbox)(?:=|$)/.test(arg)),
      'Browser command line disabled a sandbox boundary');
    result.browserCommandLine = command.arguments;
    await session.detach();
  }
  context = await browser.newContext({viewport: {width: 1440, height: 1000}});
  page = await context.newPage();
  let rejectFailure;
  const pageFailure = new Promise((_, reject) => { rejectFailure = reject; });
  pageFailure.catch(() => {});
  const fail = error => {
    result.pageErrors.push(String(error));
    rejectFailure(error instanceof Error ? error : new Error(String(error)));
  };
  page.on('pageerror', fail);
  page.on('crash', () => fail(new Error('Browser page crashed')));
  page.on('console', message => {
    if (result.browserConsole.length < 2048) result.browserConsole.push({type: message.type(), text: message.text()});
  });
  context.on('request', request => {
    const requested = new URL(request.url());
    if (result.requests.length >= 1024) { fail(new Error('Unexpected browser request volume')); return; }
    result.requests.push({url: request.url(), method: request.method(), resourceType: request.resourceType()});
    if (!['blob:', 'data:'].includes(requested.protocol) && requested.origin !== origin) {
      fail(new Error('The browser attempted an external request: ' + requested.origin));
    }
    if (!['GET', 'HEAD'].includes(request.method())) fail(new Error('The host must serve static files only'));
  });
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error('The complete browser concept check exceeded 20 minutes')), 20 * 60000);
  });
  deadline.catch(() => {});
  const wait = operation => Promise.race([pageFailure, deadline, operation]);
  await wait(page.goto(url));
  await wait(page.waitForFunction(() => window.conceptEvidence?.failed ||
    (crossOriginIsolated && !document.getElementById('start').disabled), null, {timeout: 45000}));
  const before = await page.evaluate(() => ({isolated: crossOriginIsolated,
    evidence: window.conceptEvidence, sequence: window.labLogSequence, userAgent: navigator.userAgent}));
  assert.equal(before.isolated, true);
  assert.equal(before.evidence.failed, false, before.evidence.error);
  assert.equal(before.evidence.started, false);
  assert.equal(before.sequence, 0);
  assert(!result.requests.some(request => /\/(?:out\.js|qemu-system-x86_64\.(?:data|wasm)|concept|openfga)$/.test(new URL(request.url).pathname)),
    'An executable was requested before the user started the playground');
  result.userAgent = before.userAgent;
  await wait(page.screenshot({path: path.join(output, 'before-start.png'), fullPage: true}));
  await page.exposeBinding('__recordBrowserLabLog', (_source, batch) => {
    try { guestLog.appendBatch(batch); }
    catch (error) { fail(error); throw error; }
  });
  await page.exposeBinding('__reportBrowserLabLogFailure', (_source, message) => fail(new Error(message)));
  await page.evaluate(async () => {
    const {subscribeToGuestLog} = await import('./log-stream.mjs');
    window.__finishBrowserLabLog = subscribeToGuestLog(window,
      batch => window.__recordBrowserLabLog(batch),
      {onError: error => window.__reportBrowserLabLogFailure(String(error))});
  });
  assert.equal(await page.evaluate(() => window.labLogSequence), 0);
  const launchAgeMs = Date.now() - Date.parse(timing.started_at);
  assert(launchAgeMs >= 0 && launchAgeMs <= 10 * 60000,
    'The ten-minute setup budget from the actual Actions job start expired before VM launch');
  result.vmLaunch = {at: new Date().toISOString(), actualJobAgeMs: launchAgeMs, setupLimitMs: 10 * 60000};
  await save();
  await wait(page.locator('#start').click());
  result.vmStarted = true;
  console.log(engineName + ': started one fresh concept VM');

  async function idle() {
    await wait(page.waitForFunction(() => {
      const state = window.conceptEvidence;
      return state?.failed || state?.error || (state?.ready && !state.pending &&
        !document.getElementById('read').disabled);
    }, null, {timeout: 20 * 60000}));
    const state = await wait(page.evaluate(() => window.conceptEvidence));
    assert.equal(state.failed, false, state.error);
    assert.equal(state.error, undefined);
    assert.equal(state.ready, true);
    assert.equal(state.pending, null);
    assert(state.observations.alice && state.observations.bob,
      'The completed UI action lacks successful permission observations: ' + JSON.stringify(state.responses.slice(-4)));
    return state;
  }
  const initial = await idle();
  result.readyAt = new Date().toISOString();
  assert.deepEqual(initial.startupManifest, manifest);
  assert.equal(initial.health.model_sha256, modelSha256);
  assert.equal(initial.health.document, 'demo');
  assert.equal(initial.health.owner, 'alice');
  assert.equal(initial.vm.cpus, 2);
  assert.equal(initial.vm.memoryMiB, 512);
  assert.equal(initial.vm.kernelLogLevel, 7);
  assert.equal(initial.vm.qemuLiveInstanceLimit, engineName === 'firefox' ? 8192 : 15000);
  assert(initial.vm.reportedLogicalCPUs >= 2);
  result.health = initial.health;
  result.vm = initial.vm;
  result.initialObservations = initial.observations;
  const fixture = await readJSON('browser-concept/fixtures.json');

  function permissions(state, bobCanRead) {
    assert.equal(state.observations.alice.can_read, true);
    assert.equal(state.observations.alice.can_share, true);
    assert.equal(state.observations.bob.can_read, bobCanRead);
    assert.equal(state.observations.bob.can_share, false);
    for (const actor of ['alice', 'bob']) {
      const observation = state.observations[actor];
      const response = state.responses.find(item => item.id === observation.requestId);
      assert(response && response.actor === actor && response.operation === 'permissions' && response.http_status === 200);
      assert.equal(response.error, undefined);
    }
  }
  permissions(initial, false);
  async function action(actor, operation, status) {
    const beforeCount = (await idle()).responses.length;
    await wait(page.locator(`label.person:has(input[name="actor"][value="${actor}"])`).click());
    assert.equal(await page.locator(`input[name="actor"][value="${actor}"]`).isChecked(), true);
    await wait(page.locator('#' + operation).click());
    const after = await idle();
    const response = after.responses[beforeCount];
    assert(response, 'The button did not produce an actual guest response');
    assert.equal(response.actor, actor);
    assert.equal(response.operation, operation);
    assert.equal(response.http_status, status, `${actor} ${operation} did not return HTTP ${status}`);
    assert.equal(response.error, undefined);
    if (status === 403) assert.deepEqual(response.body, {error: 'forbidden'});
    const rows = await page.locator('#history tr').evaluateAll(elements => elements.map(element => ({...element.dataset})));
    const row = rows.find(item => Number(item.requestId) === response.id);
    assert.deepEqual(row, {requestId: String(response.id), actor, operation, httpStatus: String(status)});
    if (operation === 'read') {
      if (status === 200) {
        assert.deepEqual(response.body, fixture.document);
        assert.equal(await page.locator('#document-body').textContent(), fixture.document.body);
      } else {
        assert.equal(after.document, null);
        assert(!(await page.locator('#document-body').textContent()).includes(fixture.document.body));
      }
    }
    console.log(`${engineName}: #${response.id} ${actor} ${operation} -> ${status}`);
    return {response, state: after};
  }
  for (const operation of ['grant', 'revoke']) {
    const denied = await action('bob', operation, 403);
    permissions(denied.state, false);
    result.negativeControls.push({phase: 'before-grant', requestId: denied.response.id, actor: 'bob', operation, status: 403});
  }
  for (let cycle = 1; cycle <= 2; cycle++) {
    const observed = [];
    observed.push((await action('bob', 'read', 403)).response);
    const grant = await action('alice', 'grant', 204);
    observed.push(grant.response);
    permissions(grant.state, true);
    assert.equal(grant.state.lastRead.actor, 'bob');
    assert.equal(grant.state.lastRead.http_status, 200);
    observed.push((await action('bob', 'read', 200)).response);
    if (cycle === 1) await wait(page.screenshot({path: path.join(output, 'viewer-granted.png'), fullPage: true}));
    for (const operation of ['grant', 'revoke']) {
      const denied = await action('bob', operation, 403);
      permissions(denied.state, true);
      assert.equal(denied.state.lastRead.http_status, 200);
      result.negativeControls.push({phase: 'viewer', cycle, requestId: denied.response.id, actor: 'bob', operation, status: 403});
    }
    const revoke = await action('alice', 'revoke', 204);
    observed.push(revoke.response);
    permissions(revoke.state, false);
    assert.equal(revoke.state.lastRead.actor, 'bob');
    assert.equal(revoke.state.lastRead.http_status, 403);
    observed.push((await action('bob', 'read', 403)).response);
    assert.deepEqual(observed.map(item => item.http_status), expectedStatuses);
    result.cycles.push({id: cycle, requestIds: observed.map(item => item.id), statuses: observed.map(item => item.http_status)});
  }
  const final = await idle();
  permissions(final, false);
  for (let i = 0; i < final.responses.length; i++) {
    const response = final.responses[i];
    assert.equal(response.id, i + 1);
    assert.equal(response.error, undefined);
    assert([200, 204, 403].includes(response.http_status));
  }
  assert.equal(final.document, null);
  const badges = await page.locator('#alice-read, #alice-share, #bob-read, #bob-share')
    .evaluateAll(elements => Object.fromEntries(elements.map(element => [element.id, element.textContent])));
  assert.deepEqual(badges, {'alice-read': 'Allowed', 'alice-share': 'Allowed', 'bob-read': 'Denied', 'bob-share': 'Denied'});
  assert.deepEqual(result.pageErrors, []);
  result.observedRequests = final.responses.length;
  checksPassed = true;
} catch (error) {
  result.failure = String(error);
  console.error(engineName + ': concept browser check failed: ' + error);
} finally {
  clearTimeout(timer);
  if (page && guestLog) {
    const captureDeadline = Date.now() + 10000;
    const remaining = () => Math.max(0, captureDeadline - Date.now());
    let capture = await drainGuestLogWithin(page.evaluate(async () => {
      const {freezeGuestLogCapture} = await import('./log-stream.mjs');
      return freezeGuestLogCapture(window);
    }), remaining()).catch(error => ({error: String(error)}));
    if (!capture.error) {
      const drain = await drainGuestLogWithin(page.evaluate(() => window.__browserLabLogDrain), remaining())
        .catch(error => ({error: String(error)}));
      capture = {...capture, ...drain};
    }
    result.guest = capture.guest ?? {error: capture.error ?? 'Guest state was unavailable at the capture cutoff'};
    const captured = guestLog.snapshot(capture.expectedSequence);
    result.logCapture = {...captured.metadata, ...(capture.error ? {error: capture.error} : {})};
    if (capture.error || capture.deliveredSequence !== capture.expectedSequence) result.logCapture.complete = false;
    await fs.writeFile(path.join(output, 'console.txt'), captured.text);
    await fs.writeFile(path.join(output, 'stream.jsonl'), captured.jsonl);
    result.logFiles = {console: await record(path.join(output, 'console.txt')), stream: await record(path.join(output, 'stream.jsonl'))};
    await fs.writeFile(path.join(output, 'console-tail.txt'),
      await page.locator('#log').textContent({timeout: 5000}).catch(error => String(error)));
    await page.screenshot({path: path.join(output, 'final.png'), fullPage: true, timeout: 10000})
      .catch(error => { result.screenshotError = String(error); });
    if (checksPassed) {
      if (!result.logCapture.complete || !result.guest.ready || result.guest.failed || result.guest.error ||
          result.pageErrors.length || result.screenshotError) {
        result.failure = 'Guest, browser, screenshot or complete log capture failed after the final action';
        checksPassed = false;
      }
      if (!captured.text.includes('[concept-guest] online CPUs: 0-1; service CPUs: 0-1')) {
        result.failure = 'The guest did not provide actual two-CPU discovery evidence';
        checksPassed = false;
      }
    }
  }
  if (browser) await browser.close().catch(error => { checksPassed = false; result.closeError = String(error); });
  if (server) {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
  result.passed = checksPassed;
  result.finishedAt = new Date().toISOString();
  await save();
}
if (!result.passed) process.exitCode = 1;
console.log(JSON.stringify({engine: engineName, passed: result.passed, cycles: result.cycles,
  negativeControls: result.negativeControls.length, logCaptureComplete: result.logCapture?.complete,
  failure: result.failure, output}));
