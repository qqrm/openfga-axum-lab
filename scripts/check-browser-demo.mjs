import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const bytes = await readFile("browser-output/domain.wasm");
const { instance } = await WebAssembly.instantiate(bytes);
const { is_visible, next_state } = instance.exports;
assert.deepEqual([0, 1, 2, 3, 4].map(is_visible), [0, 1, 0, 0, 0]);
assert.equal(next_state(0, 0), 1);
assert.equal(next_state(1, 1), 2);
assert.equal(next_state(2, 2), 3);
assert.equal(next_state(3, 0), -1);
assert.equal(next_state(1, 3), -1);
assert.equal(next_state(4, 4), -1);
assert.equal(is_visible(4), 0);
console.log("Browser WASM domain checks passed");
