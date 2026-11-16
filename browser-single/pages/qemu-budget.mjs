// Firefox's observed module-allocation failure needs an earlier admission cap.
// This changes only QEMU compilation admission, never guest authorization.
export function configureInstanceBudget(module, userAgent) {
  const limit = /\bFirefox\/\d+/.test(userAgent) ? 8192 : 15000;
  if (typeof module?._qemu_wasm_set_instance_limit !== 'function') {
    throw new Error('The verified emulator instance-budget export is missing');
  }
  const applied = module._qemu_wasm_set_instance_limit(limit);
  if (applied !== limit) {
    throw new Error('The emulator refused its startup instance budget');
  }
  return applied;
}
