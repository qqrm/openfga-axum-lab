import assert from 'node:assert/strict';
import test from 'node:test';
import {configureInstanceBudget} from './pages/qemu-budget.mjs';

test('startup selects the observed Firefox cap and preserves the other-engine default', () => {
  for (const [userAgent, expected] of [
    ['Mozilla/5.0 Gecko/20100101 Firefox/159.0', 8192],
    ['Mozilla/5.0 Chrome/158.0.8127.18 Safari/537.36', 15000],
    ['', 15000],
    ['NotFirefox/159.0', 15000],
  ]) {
    const calls = [];
    const module = {_qemu_wasm_set_instance_limit(value) { calls.push(value); return value; }};
    assert.equal(configureInstanceBudget(module, userAgent), expected);
    assert.deepEqual(calls, [expected]);
  }
});

test('startup refuses a missing, rejected or mismatched emulator configuration', () => {
  assert.throws(() => configureInstanceBudget({}, 'Firefox/159.0'), /export is missing/);
  for (const response of [0, 15000, '8192', undefined]) {
    assert.throws(() => configureInstanceBudget({
      _qemu_wasm_set_instance_limit() { return response; },
    }, 'Firefox/159.0'), /refused its startup instance budget/);
  }
  const failure = new Error('emulator trap');
  assert.throws(() => configureInstanceBudget({
    _qemu_wasm_set_instance_limit() { throw failure; },
  }, 'Firefox/159.0'), error => error === failure);
});
