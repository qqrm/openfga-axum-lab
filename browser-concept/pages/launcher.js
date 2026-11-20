import {actors, beginCommand, consumeConceptLine, createSerialLineDecoder, initialConceptEvidence} from './protocol.mjs';
import {loadStartupManifest, loadStartupPayload} from './startup-payload.mjs';
import {observeRuntimeErrors} from './runtime-errors.mjs';
import {createTranscriptLog, createCoalescedTranscriptRender, guestLogEvent} from './log-stream.mjs';
import {configureInstanceBudget} from './qemu-budget.mjs';

const element = id => document.getElementById(id);
const names = {alice: 'Alice', bob: 'Bob'};
const actionNames = {permissions: 'Check permissions', read: 'Read document', grant: 'Grant viewer', revoke: 'Revoke viewer'};
const evidence = window.conceptEvidence = initialConceptEvidence();
// Reuse the verified source collector without claiming Lakekeeper acceptance.
window.labEvidence = evidence;
window.labLogSequence = 0;
let started = false, failed = false, isolated = false, busy = false;
let preparedManifest, bootTimer, pendingRequest;
let selectedActor = 'bob';
const input = [], readable = new Set(), signals = new Set();
const transcriptRender = createCoalescedTranscriptRender(text => { element('log').textContent = text; }, fail);
window.__flushBrowserLabTranscript = transcriptRender.flush;
const log = createTranscriptLog(transcriptRender.render, chunk => {
  window.labLogSequence = chunk.sequence;
  window.dispatchEvent(new CustomEvent(guestLogEvent, {detail: chunk}));
});

function phase(title, detail) {
  element('phase').textContent = title;
  element('detail').textContent = detail;
  const label = element('runtime-state');
  label.textContent = failed ? 'Stopped' : evidence.ready ? 'Running' : started ? 'Starting' : 'Not running';
  label.className = 'state-label' + (failed ? ' failed' : evidence.ready ? ' running' : '');
  document.querySelector('.status-dot').className = 'status-dot' + (failed ? ' failed' : evidence.ready ? ' running' : '');
}

function fail(error) {
  if (failed) return;
  failed = true;
  clearTimeout(bootTimer);
  evidence.failed = true;
  evidence.ready = false;
  evidence.error = String(error);
  evidence.observations = {alice: null, bob: null};
  evidence.document = null;
  evidence.lastRead = null;
  if (pendingRequest) {
    clearTimeout(pendingRequest.timer);
    pendingRequest.reject(error instanceof Error ? error : new Error(String(error)));
    pendingRequest = null;
  }
  phase('This run has stopped', 'Reset the tab to start a new isolated playground.');
  message(String(error), true);
  render();
  log('\nCONCEPT_UI_ERROR: ' + String(error) + '\n');
}
observeRuntimeErrors(window, fail);

function message(text, error = false) {
  element('action-message').textContent = text;
  element('action-message').className = 'action-message' + (error ? ' error' : '');
}

function timeLabel(value) {
  return new Date(value).toLocaleTimeString([], {hour: '2-digit', minute: '2-digit', second: '2-digit'});
}

function render() {
  const canAct = evidence.ready && !failed && !busy && !evidence.pending;
  element('start').disabled = started || failed || !isolated || !preparedManifest;
  element('actor-picker').disabled = !canAct;
  for (const operation of ['permissions', 'read', 'grant', 'revoke']) element(operation).disabled = !canAct;
  element('read').textContent = 'Read as ' + names[selectedActor] + ' →';
  const acting = element('acting-as');
  acting.replaceChildren();
  const avatar = document.createElement('span');
  avatar.className = 'tiny-avatar avatar-' + selectedActor;
  avatar.textContent = names[selectedActor][0];
  avatar.setAttribute('aria-hidden', 'true');
  acting.append(avatar, document.createTextNode('Using ' + names[selectedActor] + '’s demo session'));
  for (const actor of actors) {
    const observed = evidence.observations[actor];
    for (const [relation, suffix] of [['can_read', 'read'], ['can_share', 'share']]) {
      const output = element(actor + '-' + suffix);
      output.textContent = observed ? (observed[relation] ? 'Allowed' : 'Denied') :
        evidence.pending?.operation === 'permissions' && evidence.pending.actor === actor ? 'Checking…' : 'Not checked';
      output.className = 'permission ' + (observed ? observed[relation] ? 'allowed' : 'denied' : 'unknown');
    }
    element(actor + '-observed').textContent = observed ?
      'Observed ' + timeLabel(observed.receivedAt) + ' · request #' + observed.requestId :
      failed ? 'No current observation after this run stopped' : 'Waiting for a server observation';
  }
  renderDocument();
}

function renderDocument() {
  const content = element('document-content');
  const last = evidence.lastRead;
  content.className = 'document-content';
  element('document-title').textContent = evidence.document?.title ?? evidence.health?.title ?? 'Browser authorization demo';
  element('read-receipt').hidden = !last;
  if (!last) {
    element('document-state').textContent = failed ? 'No current document response' : 'What will ' + names[selectedActor] + ' see?';
    element('document-body').textContent = failed ? 'The run stopped. Reset before making another request.' :
      evidence.ready ? 'Read the document to see the server’s answer for ' + names[selectedActor] + '.' :
      started ? 'Waiting for the real backend to finish starting.' : 'Start the playground, then try reading the document as Bob.';
    return;
  }
  element('read-receipt').textContent = names[last.actor] + ' · request #' + last.id + ' · ' +
    (last.http_status === null ? 'No HTTP response' : 'HTTP ' + last.http_status) + ' · observed ' + timeLabel(last.receivedAt);
  if (last.error !== undefined) {
    content.classList.add('error');
    element('document-state').textContent = 'No complete document response';
    element('document-body').textContent = last.error;
  } else if (last.http_status === 200 && evidence.document) {
    content.classList.add('loaded');
    element('document-state').textContent = names[last.actor] + ' received the document';
    element('document-body').textContent = evidence.document.body;
  } else if (last.http_status === 403) {
    content.classList.add('denied');
    element('document-state').textContent = 'Access denied for ' + names[last.actor];
    element('document-body').textContent = 'The server returned HTTP 403. It did not return the document contents.';
  } else {
    content.classList.add('error');
    element('document-state').textContent = 'The document was not returned';
    element('document-body').textContent = 'The server returned HTTP ' + last.http_status +
      (typeof last.body?.error === 'string' ? ': ' + last.body.error : '.');
  }
}

function appendReceipt(response) {
  const row = document.createElement('tr');
  row.dataset.requestId = String(response.id);
  row.dataset.actor = response.actor;
  row.dataset.operation = response.operation;
  row.dataset.httpStatus = response.http_status === null ? 'none' : String(response.http_status);
  const values = ['#' + response.id, names[response.actor], actionNames[response.operation]];
  for (const value of values) {
    const cell = document.createElement('td'); cell.textContent = value; row.append(cell);
  }
  const statusCell = document.createElement('td');
  const status = document.createElement('span');
  status.className = 'http-status ' + (response.error !== undefined || response.http_status === null || response.http_status >= 500 ? 'error' :
    response.http_status === 403 ? 'denied' : response.http_status >= 200 && response.http_status < 300 ? 'ok' : 'error');
  status.textContent = response.http_status === null ? 'No response' : 'HTTP ' + response.http_status;
  statusCell.append(status);
  const explanation = response.error ?? (typeof response.body?.error === 'string' ? response.body.error : null);
  if (explanation) {
    const detail = document.createElement('span'); detail.className = 'response-detail';
    detail.textContent = explanation.length > 150 ? explanation.slice(0, 150) + '…' : explanation;
    statusCell.append(detail);
  }
  row.append(statusCell);
  const elapsed = document.createElement('td'); elapsed.className = 'duration';
  elapsed.textContent = response.elapsed_ms < 1000 ? response.elapsed_ms + ' ms' : (response.elapsed_ms / 1000).toFixed(2) + ' s';
  elapsed.title = 'Elapsed time reported by the guest HTTP bridge'; row.append(elapsed);
  row.title = 'Received ' + response.receivedAt + (response.body !== null ? '\n' + JSON.stringify(response.body) : '');
  element('history').append(row);
  element('history-empty').hidden = true;
  element('history-scroll').hidden = false;
  element('response-count').textContent = evidence.responses.length + (evidence.responses.length === 1 ? ' request' : ' requests');
  element('history-scroll').scrollTop = element('history-scroll').scrollHeight;
}

function describe(response) {
  const action = names[response.actor] + ' · ' + actionNames[response.operation] + ': ';
  if (response.error !== undefined) return action + (response.http_status === null ? 'no HTTP response. ' : 'HTTP ' + response.http_status + ', response incomplete. ') + response.error;
  return action + 'HTTP ' + response.http_status + '.';
}

function request(operation, actor) {
  return new Promise((resolve, reject) => {
    let wire;
    try { wire = beginCommand(evidence, operation, actor); }
    catch (error) { reject(error); return; }
    pendingRequest = {resolve, reject, timer: setTimeout(() => fail('No complete guest response arrived within 30 seconds. The operation’s outcome is unknown; it will not be retried.'), 30000)};
    input.push(...new TextEncoder().encode(wire));
    render();
    for (const callback of readable) callback();
  });
}

async function runAction(operation) {
  if (busy || failed || !evidence.ready) return;
  busy = true; render();
  const actor = selectedActor;
  message('Waiting for ' + names[actor] + '’s ' + actionNames[operation].toLowerCase() + ' response…');
  try {
    const primary = await request(operation, actor);
    if (operation === 'grant' || operation === 'revoke') {
      // Refresh actual observations after either an accepted or rejected
      // mutation. Never retry the write or infer a badge from HTTP 204.
      await request('permissions', 'alice');
      await request('permissions', 'bob');
      await request('read', 'bob');
    } else if (operation === 'read') {
      await request('permissions', actor);
    }
    message(describe(primary) + (operation === 'grant' || operation === 'revoke' ? ' Bob’s permissions and read were checked again.' : ''),
      primary.error !== undefined || primary.http_status === null || primary.http_status >= 500);
  } catch (error) { fail(error); }
  finally { busy = false; render(); }
}

async function initialObservations() {
  busy = true; render();
  try {
    const alice = await request('permissions', 'alice');
    const bob = await request('permissions', 'bob');
    if ([alice, bob].every(response => response.http_status === 200 && response.error === undefined)) {
      message('Permissions observed. Try reading as Bob, then switch to Alice to grant access.');
    } else {
      message('The backend is running, but a permission observation failed. See the actual response below; unchecked permissions remain unknown.', true);
    }
  } catch (error) { fail(error); }
  finally { busy = false; render(); }
}

function line(value) {
  if (failed) return;
  try {
    const event = consumeConceptLine(evidence, value);
    if (!event) return;
    if (event.kind === 'phase' && !evidence.ready) {
      const title = event.value.phase.replaceAll('-', ' ').replaceAll('_', ' ');
      phase(title[0].toUpperCase() + title.slice(1), event.value.detail ?? 'Starting the real services inside this tab.');
    }
    if (event.kind === 'ready') {
      clearTimeout(bootTimer);
      phase('Your backend is running', 'Checking the current permissions through the real HTTP API.');
      element('backend-identity').textContent = 'OpenFGA store ' + evidence.health.store_id + ' · model ' + evidence.health.model_id +
        ' · model SHA-256 ' + evidence.health.model_sha256 + ' · source ' + evidence.startupManifest.conceptSourceCommit;
      void initialObservations();
    }
    if (event.kind === 'response') {
      appendReceipt(event.value);
      const waiting = pendingRequest;
      if (!waiting) throw new Error('No active host request matches the guest response');
      pendingRequest = null; clearTimeout(waiting.timer); waiting.resolve(event.value);
      render();
    }
  } catch (error) { fail(error); }
}

const serial = createSerialLineDecoder(line);
const rawSerialDecoder = new TextDecoder();
window.__conceptPendingSerialBytes = () => serial.pendingBytes;
let termios = {iflag: 0, oflag: 0, cflag: 0, lflag: 0, cc: Array(32).fill(0)};
const pty = {
  get readable() { return input.length > 0; }, get writable() { return true; },
  read(length) { return input.splice(0, length); },
  write(bytes) {
    // Preserve the exact source order even when parsing the current frame
    // fails or triggers the next request. Telemetry never enters this parser.
    log(rawSerialDecoder.decode(new Uint8Array(bytes), {stream: true}));
    try { serial.write(bytes); }
    catch (error) { fail(error); }
  },
  onReadable(callback) { readable.add(callback); return {dispose() { readable.delete(callback); }}; },
  onSignal(callback) { signals.add(callback); return {dispose() { signals.delete(callback); }}; },
  ioctl(command, value) {
    if (command === 'TCGETS') return termios;
    if (command === 'TCSETS') { termios = value; return; }
    if (command === 'TIOCGWINSZ') return [120, 40];
    throw new Error('Unsupported terminal operation: ' + command);
  },
};
function script(filename) {
  return new Promise((resolve, reject) => {
    const node = document.createElement('script');
    node.src = new URL(filename, import.meta.url).href;
    node.onload = resolve; node.onerror = () => reject(new Error('Cannot load ' + filename));
    document.head.append(node);
  });
}

async function start() {
  if (started || !crossOriginIsolated || !preparedManifest) return;
  started = true;
  evidence.started = true;
  bootTimer = setTimeout(() => fail('The backend did not become ready within 20 minutes. Reset the tab to stop this run.'), 1200000);
  element('start').disabled = true;
  phase('Loading Linux image and emulator', 'Large files are loading locally. No external backend is contacted.');
  try {
    // This pinned backend sizes its per-CPU vectors from the browser's report.
    // Reject insufficient capacity before loading or requesting two vCPUs.
    const reportedLogicalCPUs = navigator.hardwareConcurrency;
    if (!Number.isSafeInteger(reportedLogicalCPUs) || reportedLogicalCPUs < 2) {
      throw new Error('This experiment requires the browser to report at least two logical CPUs.');
    }
    const {manifest, payload} = await loadStartupPayload(import.meta.url);
    evidence.source = manifest.conceptSourceCommit;
    evidence.expectedModelSha256 = manifest.model.sha256;
    window.labEvidence.startupManifest = manifest;
    const module = window.Module = {
      preRun: [], pty,
      locateFile: name => new URL(`runtime/${name}`, import.meta.url).href,
      mainScriptUrlOrBlob: new URL('runtime/out.js', import.meta.url).href,
      print: text => log(`${text}\n`),
      printErr: text => log(`${text}\n`),
      setStatus: text => { if (!window.labEvidence.ready && text) element('detail').textContent = text; },
      onAbort: error => fail(`WebAssembly aborted: ${error}`),
      onExit: code => { if (!failed) fail(`Guest emulator exited with status ${code}`); }
    };
    await script('runtime/load.js');
    await script('runtime/arg-module.js');
    const args = module.arguments;
    const snapshot = args.indexOf('-incoming');
    if (snapshot < 0 || args[args.indexOf('-m') + 1] !== '128M') throw new Error('Runtime argument layout differs from the reviewed artifact');
    // The saved converter VM has only 128 MiB. Cold-boot the same Linux image
    // with 512 MiB instead of restoring an incompatible memory snapshot.
    args.splice(snapshot, 2);
    args[args.indexOf('-m') + 1] = '512M';
    // Keep the reviewed two-socket topology while enabling ACPI discovery.
    // guest.sh independently requires both CPUs online before starting services.
    const smp = args.indexOf('-smp');
    const commandLine = args.indexOf('-append');
    if (smp < 0 || args[smp + 1] !== '1,sockets=1' || commandLine < 0) throw new Error('Unexpected reviewed CPU/kernel arguments');
    const kernelFlags = args[commandLine + 1].split(/\s+/);
    if (kernelFlags.filter(flag => flag === 'slub_debug=F').length !== 1) throw new Error('Missing reviewed allocator debug option');
    const initSeparator = kernelFlags.indexOf('--');
    if (kernelFlags.filter(flag => flag === 'loglevel=0').length !== 1 ||
        initSeparator < 0 || kernelFlags.indexOf('loglevel=0') >= initSeparator) {
      throw new Error('Unexpected reviewed kernel log-level option');
    }
    if (kernelFlags.filter(flag => flag === 'acpi=off').length !== 1 ||
        kernelFlags.indexOf('acpi=off') >= initSeparator) {
      throw new Error('Unexpected reviewed kernel ACPI option');
    }
    if (args.includes('-no-acpi') || args.includes('-no-hpet')) {
      throw new Error('Unexpected disabled firmware timer discovery');
    }
    args[smp + 1] = '2,sockets=2,cores=1,threads=1';
    args[commandLine + 1] = kernelFlags.filter(flag => flag !== 'slub_debug=F' && flag !== 'acpi=off')
      .map(flag => flag === 'loglevel=0' ? 'loglevel=7' : flag).join(' ');
    window.labEvidence.vm = {cpus:2, reportedLogicalCPUs, cpuTopology:'2,sockets=2,cores=1,threads=1', memoryMiB:512, slabDebug:false, kernelLogLevel:7, qemuLiveInstanceLimit:null};
    // Keep the explicit virtio NIC, attached only to an isolated guest hub.
    // Removing it makes this QEMU build enumerate default NICs and trap in
    // g_slist_sort_real. A hub has no host socket, proxy or external backend.
    const network = args.indexOf('-netdev');
    if (network < 0) throw new Error('Missing reviewed virtual network argument');
    args[network + 1] = 'hubport,id=vmnic,hubid=0';
    // Emscripten's host /dev/urandom reads Web Crypto bytes. Linux's VirtIO
    // RNG driver credits them; getrandom retains its normal readiness checks.
    if (typeof globalThis.crypto?.getRandomValues !== 'function') throw new Error('Web Crypto is required for guest entropy');
    if (args.some(arg => typeof arg === 'string' &&
        (arg === '-seed' || arg.startsWith('-seed=') || arg.includes('browser_rng') ||
         /^(?:rng-(?:random|builtin|egd)|virtio-rng)/.test(arg)))) {
      throw new Error('Unexpected existing or deterministic guest entropy configuration');
    }
    args.push('-object', 'rng-random,filename=/dev/urandom,id=browser_rng',
      '-device', 'virtio-rng-pci,rng=browser_rng');
    window.labEvidence.vm.entropyConfiguration = {backend:'rng-random',hostDevice:'/dev/urandom',guestDevice:'virtio-rng-pci',webCryptoRequired:true};
    module.preRun.push(mod => {
      // The exported setter writes shared C state before main starts any CPU.
      // Record the returned value, and stop if this binary refuses the policy.
      window.labEvidence.vm.qemuLiveInstanceLimit = configureInstanceBudget(mod, navigator.userAgent);
      mod.FS.mkdir('/browser-host');
      for (const file of payload) {
        mod.FS.writeFile(`/browser-host/${file.name}`, file.bytes, {mode:0o555});
      }
      mod.FS.writeFile('/pack/info', `t:${Math.round(Date.now() / 1000)}\nmr: browser-host\ne: /bin/bash\nc: /browser-host/guest.sh\n`);
    });
    let callbacks = [];
    pty.onReadable(() => { const pending = callbacks; callbacks = []; for (const callback of pending) callback(); });
    module.preRun.push(mod => {
      mod.TTY.stream_ops.poll = (_stream, _timeout, callback) => {
        if (pty.readable) return 1;
        if (callback) {
          callback.registerCleanupFunc(() => { callbacks = callbacks.filter(item => item !== callback); });
          callbacks.push(callback);
        }
        return 0;
      };
    });
    const {default: initialize} = await import('./runtime/out.js');
    await initialize(module);
    // Initialization alone is deliberately not marked ready or accepted.
  } catch (error) { fail(error.message); }
}

element('start').addEventListener('click', () => { void start(); });
element('reset').addEventListener('click', () => location.reload());
for (const operation of ['permissions', 'read', 'grant', 'revoke']) {
  element(operation).addEventListener('click', () => { void runAction(operation); });
}
for (const radio of document.querySelectorAll('input[name="actor"]')) {
  radio.addEventListener('change', () => {
    if (!radio.checked || busy || evidence.pending || !actors.includes(radio.value)) return;
    selectedActor = radio.value;
    evidence.document = null; evidence.lastRead = null;
    message('Using ' + names[selectedActor] + '’s demo session. Choose an action to make a real request.');
    render();
  });
}

async function isolate() {
  if (crossOriginIsolated && typeof SharedArrayBuffer === 'function') {
    sessionStorage.removeItem('authz-concept-isolation-reload');
    isolated = true;
    element('isolation').textContent = 'Browser isolation is ready. The VM has not started.';
    render();
    return;
  }
  if (!isSecureContext || !('serviceWorker' in navigator)) throw new Error('A secure context with service-worker support is required.');
  await navigator.serviceWorker.register(new URL('isolation-sw.js', import.meta.url), {scope: './'});
  await navigator.serviceWorker.ready;
  if (!navigator.serviceWorker.controller) {
    await new Promise(resolve => navigator.serviceWorker.addEventListener('controllerchange', resolve, {once: true}));
  }
  if (sessionStorage.getItem('authz-concept-isolation-reload')) throw new Error('This browser did not enable isolation. No VM has started.');
  sessionStorage.setItem('authz-concept-isolation-reload', '1');
  location.reload();
}
async function prepare() {
  await isolate();
  if (!isolated) return;
  preparedManifest = await loadStartupManifest(import.meta.url);
  element('download-size').textContent = 'Starting downloads about ' + Math.ceil(preparedManifest.downloadBytes / 1024**2) + ' MiB.';
  render();
}
render();
prepare().catch(error => fail(error));
