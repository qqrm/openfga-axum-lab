#!/usr/bin/env node
// Test actual EM_JS bodies with real small Wasm; callbacks are delivered by the
// fixture, so this does not test browser GC timing or browser acceptance.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

assert.equal(process.argv.length, 3, `usage: ${process.argv[1]} /path/to/qemu/tcg/wasm32.c`);
const source = readFileSync(process.argv[2], 'utf8');
const names = ['init_wasm32_js', 'instantiate_wasm', 'remove_module_js'];
const extracted = names.map(name => {
  const pattern = new RegExp(`EM_JS\\([^,]+, ${name}, \\(([^)]*)\\), \\{([\\s\\S]*?)\\n[ \\t]{0,4}\\}\\);`, 'g');
  const matches = [...source.matchAll(pattern)];
  assert.equal(matches.length, 1, `Expected exactly one ${name}`);
  const [, declaration, body] = matches[0];
  const parameters = declaration ? declaration.split(',').map(p => p.trim().split(' ').at(-1)) : [];
  return `function ${name}(${parameters.join(',')}) {${body}\n}`;
}).join('\n');
const wasmBytes = Uint8Array.from(Buffer.from(
  '0061736d0100000001060160017f017f0302010007090105737461727400000a0601040020000b', 'hex'));
assert.ok(WebAssembly.validate(wasmBytes));
const prefix = '[browser-qemu] ';

function fixture(failure = '', loggerFails = false, thrown = new Error(failure)) {
  const wasmMemory = new WebAssembly.Memory({ initial: 1 });
  const heap = new Int8Array(wasmMemory.buffer);
  const view = new DataView(wasmMemory.buffer);
  // The real wasm32 TB layout, with empty export/counter/body vectors.
  view.setInt32(0, 256, true);
  view.setInt32(272, wasmBytes.length, true);
  heap.set(wasmBytes, 276);
  if (failure === 'imports') view.setInt32(276 + wasmBytes.length, 4, true);
  if (failure === 'copy') heap.slice = () => { throw thrown; };
  const logs = [], removed = [], held = [];
  let callback, registries = 0, functions = 0;
  const constructor = (target, phase) => new Proxy(target, {
    construct(real, args) {
      if (failure === phase) throw thrown;
      return Reflect.construct(real, args);
    },
  });
  const context = {
    Module: {}, HEAP8: heap, wasmMemory,
    DataView: constructor(DataView, 'metadata'),
    WebAssembly: {
      Module: constructor(WebAssembly.Module, 'module'),
      Instance: constructor(WebAssembly.Instance, 'instance'),
    },
    wasmTable: { get() { if (failure === 'imports') throw thrown; } },
    addFunction(fn, signature) {
      if (failure === 'add_function') throw thrown;
      assert.equal(signature, 'ii');
      assert.equal(fn(7), 7, 'Real Wasm export remains callable');
      return ++functions;
    },
    removeFunction(index) { removed.push(index); },
    FinalizationRegistry: class {
      constructor(cb) { callback = cb; registries++; }
      register(target, value) {
        if (failure === 'register') throw thrown;
        assert.ok(target instanceof WebAssembly.Instance);
        held.push(value); // Store only the unchanged held value, never the target.
      }
    },
    err(line) {
      if (loggerFails) throw new Error('diagnostic logger failed');
      assert.ok(line.startsWith(prefix));
      logs.push(JSON.parse(line.slice(prefix.length)));
    },
  };
  const api = runInNewContext(`${extracted}\n({${names.join(',')}})`, context);
  api.init_wasm32_js(0, 2, 64, 4, 8);
  assert.equal(registries, 1, 'Keep exactly the original registry');
  return { api, logs, removed, held, view, thrown, deliver: value => callback(value),
    state: context.Module.__wasm32_tb, diagnostics: context.Module.__wasm32_tb.diagnostics };
}

const success = fixture();
for (let i = 1; i <= 1023; i++) assert.equal(success.api.instantiate_wasm(), i);
assert.equal(success.logs.length, 0);
assert.equal(success.api.instantiate_wasm(), 1024);
assert.equal(success.logs.length, 1);
assert.deepEqual(success.logs[0], {
  event: 'registered', phase: 'add_function', core_id: 2, wasm_size: wasmBytes.length,
  attempted_total: 1024, module_created_total: 1024, instance_created_total: 1024,
  registered_total: 1024, detached_total: 0, gc_total: 0,
  encoded_bytes_total: 1024 * wasmBytes.length, encoded_bytes_max: wasmBytes.length,
});
assert.ok(success.held.every(value => value === 'instance'));
assert.equal(success.api.instantiate_wasm(), 1025);
assert.equal(success.logs.length, 1);
assert.ok(Object.values(success.diagnostics).every(value => typeof value === 'number'));
assert.equal(fixture().diagnostics.attempted_total, 0, 'Worker state is independent');
console.log('PASS real small Wasm, numeric per-worker counts, and sparse err snapshots');

success.view.setInt32(4, 2, true);
success.view.setInt32(64, 7, true);
success.view.setInt32(68, 9, true);
success.api.remove_module_js();
assert.deepEqual(success.removed, [7, 9]);
assert.equal(success.view.getInt32(4, true), 0);
assert.equal(success.diagnostics.detached_total, 2);
assert.equal(success.diagnostics.gc_total, 0);
assert.equal(success.view.getInt32(8, true), 0);
success.deliver('ignored');
assert.equal(success.diagnostics.gc_total, 0);
success.deliver('instance');
success.deliver('instance');
assert.equal(success.diagnostics.gc_total, 2);
assert.equal(success.view.getInt32(8, true), 2, 'Original callback increments C counter');
assert.equal(success.logs.length, 1, 'No new callback or detachment logging');
console.log('PASS original held value, detach order, and delivered callback accounting');

for (const phase of ['metadata', 'copy', 'imports', 'module', 'instance', 'register', 'add_function']) {
  for (const loggerFails of [false, true]) {
    const test = fixture(phase, loggerFails);
    assert.throws(() => test.api.instantiate_wasm(), error => error === test.thrown);
    assert.equal(test.diagnostics.attempted_total, 1);
    assert.equal(test.diagnostics.registered_total, 0);
    assert.equal(test.diagnostics.module_created_total, ['instance', 'register', 'add_function'].includes(phase) ? 1 : 0);
    assert.equal(test.diagnostics.instance_created_total, ['register', 'add_function'].includes(phase) ? 1 : 0);
    assert.equal(test.logs.length, loggerFails ? 0 : 1);
    if (!loggerFails) {
      assert.equal(test.logs[0].event, 'instantiate_throw');
      assert.equal(test.logs[0].phase, phase);
      assert.ok(!Object.hasOwn(test.logs[0], 'error'));
    }
  }
}
const primitive = fixture('module', false, 'out of memory');
assert.throws(() => primitive.api.instantiate_wasm(), error => error === 'out of memory');
const brokenSnapshot = fixture('module');
brokenSnapshot.state.snapshot = () => { throw new Error('snapshot invocation failed'); };
assert.throws(() => brokenSnapshot.api.instantiate_wasm(), error => error === brokenSnapshot.thrown);
const badLogger = fixture('', true);
badLogger.diagnostics.registered_total = 1023;
assert.equal(badLogger.api.instantiate_wasm(), 1);
assert.equal(badLogger.diagnostics.registered_total, 1024);
badLogger.diagnostics.registered_total = 1023;
badLogger.state.snapshot = brokenSnapshot.state.snapshot;
assert.equal(badLogger.api.instantiate_wasm(), 2);
console.log('PASS all failure phases rethrow identical values even when err throws');
console.log('PASS extracted-EM_JS SHA256', createHash('sha256').update(extracted).digest('hex'));
console.log('These checks do not measure executable memory, real GC scheduling, or browser acceptance.');
