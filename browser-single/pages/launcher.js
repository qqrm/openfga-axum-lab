import {consumeGuestLine, initialEvidence, requestVerification} from './acceptance-protocol.mjs';
import {loadStartupPayload} from './startup-payload.mjs';

const element = id => document.getElementById(id);
let started = false;
let failed = false;
let input = [];
let transcript = '';
let serialLine = '';
const decoder = new TextDecoder();
const readable = new Set();
const signals = new Set();
window.labEvidence = initialEvidence('99f00cc');

function log(text) {
  transcript = (transcript + text).slice(-120000);
  element('log').textContent = transcript;
}
function phase(title, detail) {
  element('phase').textContent = title;
  element('detail').textContent = detail;
}
function fail(error) {
  failed = true;
  window.labEvidence.ready = false;
  window.labEvidence.accepted = false;
  window.labEvidence.error = String(error);
  element('verify').disabled = true;
  phase('Experiment stopped', String(error));
  element('result').textContent = 'No successful result is claimed for this run. Reset the tab before retrying.';
  log(`\nERROR: ${error}\n`);
}
function line(value) {
  if (failed) return;
  try {
    const event = consumeGuestLine(window.labEvidence, value);
    if (event === 'boot') phase('Linux booted', 'Starting PostgreSQL, OpenFGA and Axum inside the guest.');
    if (event === 'ready') {
      phase('Real backend ready', 'The guest readiness check passed. Testing authorization next.');
    }
    if (event === 'start') {
      element('verify').disabled = true;
      phase('Checking real permissions', 'Running the native grant/revoke acceptance program inside this browser.');
      element('result').textContent = 'Waiting for real HTTP responses…';
    }
    if (event === 'result') {
      element('result').textContent = 'Verified inside this tab: 403 → 204 → 200 → 204 → 403';
      phase('Browser authorization check passed', 'These responses came from the real guest backend. This is not a production readiness claim.');
      element('verify').disabled = false;
    }
  } catch (error) { fail(error.message); }
}
let termios = {iflag: 0, oflag: 0, cflag: 0, lflag: 0, cc: Array(32).fill(0)};
// A raw serial channel for QEMU, not an authorization evaluator or shell emulator.
const pty = {
  get readable() { return input.length > 0; },
  get writable() { return true; },
  read(length) { return input.splice(0, length); },
  write(bytes) {
    const text = decoder.decode(new Uint8Array(bytes), {stream: true});
    log(text);
    serialLine += text.replaceAll('\r', '');
    let boundary;
    while ((boundary = serialLine.indexOf('\n')) >= 0) {
      line(serialLine.slice(0, boundary));
      serialLine = serialLine.slice(boundary + 1);
    }
    serialLine = serialLine.slice(-65536);
  },
  onReadable(callback) { readable.add(callback); return {dispose() { readable.delete(callback); }}; },
  onSignal(callback) { signals.add(callback); return {dispose() { signals.delete(callback); }}; },
  ioctl(command, value) {
    if (command === 'TCGETS') return termios;
    if (command === 'TCSETS') { termios = value; return; }
    if (command === 'TIOCGWINSZ') return [120, 40];
    throw new Error(`Unsupported terminal operation: ${command}`);
  }
};
function script(path) {
  return new Promise((resolve, reject) => {
    const node = document.createElement('script');
    node.src = new URL(path, import.meta.url).href;
    node.onload = resolve;
    node.onerror = () => reject(new Error(`Cannot load ${path}`));
    document.head.appendChild(node);
  });
}
async function start() {
  if (started || !crossOriginIsolated) return;
  started = true;
  element('start').disabled = true;
  phase('Loading Linux image and emulator', 'Large files are loading locally. No external backend is contacted.');
  try {
    const {manifest, payload} = await loadStartupPayload(import.meta.url);
    window.labEvidence.source = manifest.base_source_commit;
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
    // Candidate for the next real browser run: retain request deadlines while
    // giving Go two guest CPUs and removing the converter's allocator debugging.
    const smp = args.indexOf('-smp');
    const commandLine = args.indexOf('-append');
    if (smp < 0 || args[smp + 1] !== '1,sockets=1' || commandLine < 0) throw new Error('Unexpected reviewed CPU/kernel arguments');
    const kernelFlags = args[commandLine + 1].split(/\s+/);
    if (kernelFlags.filter(flag => flag === 'slub_debug=F').length !== 1) throw new Error('Missing reviewed allocator debug option');
    args[smp + 1] = '2,sockets=1';
    args[commandLine + 1] = kernelFlags.filter(flag => flag !== 'slub_debug=F').join(' ');
    window.labEvidence.vm = {cpus:2, memoryMiB:512, slabDebug:false};
    // Keep the explicit virtio NIC, attached only to an isolated guest hub.
    // Removing it makes this QEMU build enumerate default NICs and trap in
    // g_slist_sort_real. A hub has no host socket, proxy or external backend.
    const network = args.indexOf('-netdev');
    if (network < 0) throw new Error('Missing reviewed virtual network argument');
    args[network + 1] = 'hubport,id=vmnic,hubid=0';
    module.preRun.push(mod => {
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
element('start').addEventListener('click', start);
element('verify').addEventListener('click', () => {
  if (failed || !window.labEvidence.accepted) return;
  try { requestVerification(window.labEvidence); }
  catch (error) { fail(error.message); return; }
  element('verify').disabled = true;
  input.push(...new TextEncoder().encode('verify\n'));
  for (const callback of readable) callback();
});
element('reset').addEventListener('click', () => location.reload());

async function isolate() {
  if (crossOriginIsolated && typeof SharedArrayBuffer === 'function') {
    sessionStorage.removeItem('authz-wasm-isolation-reload');
    element('isolation').textContent = 'Browser isolation is ready. Nothing has been downloaded yet.';
    element('start').disabled = false;
    return;
  }
  if (!isSecureContext || !('serviceWorker' in navigator)) throw new Error('A secure context with service-worker support is required.');
  await navigator.serviceWorker.register(new URL('isolation-sw.js', import.meta.url), {scope: './'});
  await navigator.serviceWorker.ready;
  if (!navigator.serviceWorker.controller) {
    await new Promise(resolve => navigator.serviceWorker.addEventListener('controllerchange', resolve, {once: true}));
  }
  if (sessionStorage.getItem('authz-wasm-isolation-reload')) throw new Error('This browser did not enable cross-origin isolation. The VM has not started.');
  sessionStorage.setItem('authz-wasm-isolation-reload', '1');
  location.reload();
}
isolate().catch(error => { element('isolation').textContent = error.message; fail(error.message); });
