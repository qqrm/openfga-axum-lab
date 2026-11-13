// The loader contract follows container2wasm v0.8.5's emscripten-simple example.
// A raw PTY records serial bytes without a terminal or an external dependency.
import initEmscriptenModule from '/artifact/out.js';

const started = performance.now();
const state = globalThis.probe = {
  schema: 1,
  startedAt: new Date().toISOString(),
  userAgent: navigator.userAgent,
  crossOriginIsolated,
  sharedArrayBuffer: typeof SharedArrayBuffer !== 'undefined',
  serial: '',
  diagnostics: [],
  errors: [],
  status: 'loading',
};
const statusNode = document.querySelector('#status');
const serialNode = document.querySelector('#serial');
const diagnosticNode = document.querySelector('#diagnostics');
const elapsed = () => Math.round(performance.now() - started);
function diagnostic(kind, text) {
  const entry = {ms: elapsed(), kind, text: String(text)};
  state.diagnostics.push(entry);
  diagnosticNode.textContent += `[${entry.ms}ms] ${kind}: ${entry.text}\n`;
}
function error(kind, value) {
  const message = value?.stack || value?.message || String(value);
  state.errors.push({ms: elapsed(), kind, message});
  diagnostic(kind, message);
}
function runtimeFailure(kind, value, status = 'failed') {
  error(kind, value);
  state.failureOrigin = 'browser-runtime';
  state.status = status;
  statusNode.textContent = 'Browser/WASM runtime failed. See diagnostics.';
}
globalThis.addEventListener('error', event => runtimeFailure('window.error', event.error || event.message));
globalThis.addEventListener('unhandledrejection', event => runtimeFailure('unhandledrejection', event.reason));
// This artifact's Emscripten worker.onerror throws into the host page. The
// uncaught-error listener above and Playwright pageerror capture that failure;
// leave the native Worker constructor and artifact worker handlers untouched.

function signal() {
  const callbacks = new Set();
  return {
    listen(callback) { callbacks.add(callback); return {dispose: () => callbacks.delete(callback)}; },
    emit(value) { for (const callback of [...callbacks]) callback(value); },
  };
}
const inputReady = signal();
const terminalSignal = signal();
const input = [];
const decoder = new TextDecoder();
let termios = {iflag: 0, oflag: 0, cflag: 191, lflag: 0, cc: Array(32).fill(0)};
termios.cc[6] = 1;
const pty = {
  get readable() { return input.length > 0; },
  writable: true,
  read(length) { return input.splice(0, length); },
  write(bytes) {
    const text = decoder.decode(Uint8Array.from(bytes), {stream: true});
    if (!state.firstSerialMs) state.firstSerialMs = elapsed();
    state.serial += text;
    serialNode.textContent = state.serial;
    serialNode.scrollTop = serialNode.scrollHeight;
  },
  onReadable: callback => inputReady.listen(callback),
  onSignal: callback => terminalSignal.listen(callback),
  ioctl(operation, value) {
    if (operation === 'TCGETS') return termios;
    if (operation === 'TCSETS') { termios = value; return; }
    if (operation === 'TIOCGWINSZ') return [120, 40];
    throw new Error(`Unsupported PTY ioctl: ${operation}`);
  },
};
state.send = text => {
  input.push(...new TextEncoder().encode(text));
  inputReady.emit();
};
document.querySelector('#command').addEventListener('submit', event => {
  event.preventDefault();
  state.send(document.querySelector('#input').value + '\n');
  document.querySelector('#input').value = '';
});
document.querySelector('#download').addEventListener('click', () => {
  const link = document.createElement('a');
  link.href = URL.createObjectURL(new Blob([JSON.stringify(state, null, 2)], {type: 'application/json'}));
  link.download = 'browser-vm-probe.json';
  link.click();
  setTimeout(() => URL.revokeObjectURL(link.href), 0);
});

const options = new URLSearchParams(location.search);
const cold = options.get('boot') === 'cold';
const memory = options.get('memory');
if (memory && !cold) throw new Error('Memory override requires boot=cold because the existing snapshot fixes guest RAM.');
if (memory && !/^(128|256|512|1024)M$/.test(memory)) throw new Error('Supported diagnostic memory sizes: 128M, 256M, 512M, 1024M.');
state.mode = cold ? 'cold' : 'original-snapshot';
state.originalArguments = [...Module.arguments];
if (cold) {
  const incoming = Module.arguments.indexOf('-incoming');
  if (incoming !== -1) Module.arguments.splice(incoming, 2);
}
if (memory) Module.arguments[Module.arguments.indexOf('-m') + 1] = memory;
state.effectiveArguments = [...Module.arguments];
Module.pty = pty;
Module.mainScriptUrlOrBlob = new URL('/artifact/out.js', location).href;
Module.print = text => diagnostic('stdout', text);
Module.printErr = text => diagnostic('stderr', text);
Module.onAbort = reason => runtimeFailure('abort', reason, 'aborted');
Module.onExit = code => {
  if (!state.failureOrigin) state.status = 'exited';
  state.exitCode = code;
  diagnostic('exit', code);
};
Module.onRuntimeInitialized = () => {
  if (!state.failureOrigin) {
    state.status = 'runtime-initialized';
    statusNode.textContent = 'QEMU/WASM initialized. Watching the real guest serial console.';
  }
  state.runtimeInitializedMs = elapsed();
  diagnostic('runtime', 'initialized');
};
Module.preRun ??= [];
Module.preRun.push(mod => {
  try { mod.FS.mkdir('/pack'); } catch (e) { if (e.errno !== 20) throw e; }
  mod.FS.writeFile('/pack/info', `t:${Math.round(Date.now() / 1000)}\n`);
});
const readableCallbacks = [];
pty.onReadable(() => {
  for (const callback of readableCallbacks.splice(0)) callback();
});
Module.preRun.push(mod => {
  mod.TTY.stream_ops.poll = (_stream, _timeout, notifyCallback) => {
    if (pty.readable) return 1;
    if (notifyCallback != null) {
      notifyCallback.registerCleanupFunc(() => {
        const index = readableCallbacks.indexOf(notifyCallback);
        if (index !== -1) readableCallbacks.splice(index, 1);
      });
      readableCallbacks.push(notifyCallback);
    }
    return 0;
  };
});
diagnostic('environment', JSON.stringify({crossOriginIsolated, sharedArrayBuffer: state.sharedArrayBuffer, mode: state.mode}));
try {
  globalThis.vm = await initEmscriptenModule(Module);
  diagnostic('factory', 'resolved');
} catch (e) {
  runtimeFailure('factory', e);
}
