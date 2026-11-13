#!/usr/bin/env node
// Compile the real C budget region with the pinned SDK, then execute its
// generated ES-module glue with PROXY_TO_PTHREAD and two further pthreads.
// This is a bridge fixture, not the complete QEMU VM or browser acceptance.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

assert.ok(process.argv.length === 3 || process.argv.length === 4,
  `usage: ${process.argv[1]} /path/to/qemu/tcg/wasm32.c [PROOF_DIR]`);
const source = readFileSync(process.argv[2], 'utf8');
const start = 'int instance_alive_global = 0;';
const end = 'static void add_instance_running_local';
assert.equal(source.split(start).length, 2);
assert.equal(source.split(end).length, 2);
const region = source.slice(source.indexOf(start), source.indexOf(end));
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const blob = bytes => createHash('sha1').update(`blob ${Buffer.byteLength(bytes)}\0`).update(bytes).digest('hex');
const compiler = process.env.BROWSER_QEMU_EMCC ?? 'emcc';
const version = spawnSync(compiler, ['--version'], { encoding:'utf8', timeout:30_000 });
assert.ifError(version.error);
assert.equal(version.status, 0, version.stderr);
assert.match(version.stdout, /\b4\.0\.10\b/, 'The bridge requires the production SDK version');
const temporary = process.argv.length === 3;
const directory = temporary ? mkdtempSync(join(tmpdir(), 'qemu-budget-bridge-')) : resolve(process.argv[3]);
mkdirSync(directory, { recursive:true });
assert.deepEqual(readdirSync(directory), [], 'Use an empty proof directory');

const prefix = String.raw`
#include <emscripten.h>
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#define qatomic_read(p) __atomic_load_n(p, __ATOMIC_RELAXED)
#define qatomic_sub(p, n) ((void)__atomic_fetch_sub(p, n, __ATOMIC_SEQ_CST))
#define qatomic_cmpxchg(p, old, next) ({ int observed = (old); \
    __atomic_compare_exchange_n(p, &observed, next, false, \
                               __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST); observed; })
static void remove_module_js(void) {}
`;
const suffix = String.raw`
static int expected_limit;
static int observed_limits[2];
static void require(bool condition, const char *message) {
    if (!condition) {
        fprintf(stderr, "BRIDGE_FAILURE: %s\n", message);
        abort();
    }
}
static void *worker(void *argument) {
    int index = (int)(intptr_t)argument;
    int frozen = freeze_instance_limit();
    require(frozen == expected_limit, "pthread did not observe preRun instance limit");
    require(qemu_wasm_set_instance_limit(8192) == 0, "late 8192 configuration accepted");
    require(qemu_wasm_set_instance_limit(15000) == 0, "late 15000 configuration accepted");
    observed_limits[index] = instance_limit();
    return NULL;
}
int main(int argc, char **argv) {
    require(argc == 2, "missing expected limit");
    expected_limit = atoi(argv[1]);
    require(expected_limit == 8192 || expected_limit == 15000, "invalid fixture limit");
    require(instance_limit() == expected_limit, "main did not observe preRun instance limit");
    pthread_t workers[2];
    for (int i = 0; i < 2; i++)
        require(pthread_create(&workers[i], NULL, worker, (void *)(intptr_t)i) == 0, "pthread_create failed");
    for (int i = 0; i < 2; i++)
        require(pthread_join(workers[i], NULL) == 0, "pthread_join failed");
    require(observed_limits[0] == expected_limit && observed_limits[1] == expected_limit,
            "workers disagree on frozen limit");
    instance_alive_global = expected_limit - 1;
    require(try_reserve_instance(), "final shared slot was refused");
    require(!can_add_instance() && !try_reserve_instance(), "shared cap was exceeded");
    instance_alive_global = 0;
    require(qemu_wasm_set_instance_limit(expected_limit) == 0, "empty accounting unfreezes configuration");
    printf("BRIDGE_RESULT: {\"limit\":%d,\"workers\":[%d,%d],\"mainObservedPreRun\":true,\"lateRejected\":true}\n",
           expected_limit, observed_limits[0], observed_limits[1]);
    return 0;
}
`;
const runner = String.raw`
import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const [path, limitText, resultPath] = process.argv.slice(2);
const limit = Number(limitText);
let module, result, preRuns = 0, timer;
let resolveExit, rejectExit;
const exited = new Promise((resolve, reject) => { resolveExit = resolve; rejectExit = reject; });
try {
  const {default: initialize} = await import(pathToFileURL(resolve(path)).href);
  const initialized = initialize({
    arguments: [limitText],
    preRun: [mod => {
      module = mod;
      preRuns++;
      assert.equal(typeof mod._qemu_wasm_set_instance_limit, 'function',
        'missing qemu_wasm_set_instance_limit export');
      assert.equal(mod._qemu_wasm_set_instance_limit(4096), 0);
      assert.equal(mod._qemu_wasm_set_instance_limit(-1), 0);
      assert.equal(mod._qemu_wasm_set_instance_limit(limit), limit);
    }],
    print(line) {
      console.log(line);
      if (line.startsWith('BRIDGE_RESULT: ')) {
        result = JSON.parse(line.slice('BRIDGE_RESULT: '.length));
        // This callback runs on the original JS thread before C main returns.
        assert.equal(module._qemu_wasm_set_instance_limit(limit), 0);
        assert.equal(module._qemu_wasm_set_instance_limit(limit === 8192 ? 15000 : 8192), 0);
        result.lateJsRejected = true;
      }
    },
    printErr: line => console.error(line),
    onAbort: reason => rejectExit(new Error(String(reason))),
    onExit: code => code === 0 ? resolveExit() : rejectExit(new Error('exit ' + code)),
  });
  await Promise.race([
    Promise.all([initialized, exited]),
    new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('bridge timeout')), 15_000); }),
  ]);
  assert.equal(preRuns, 1);
  assert.deepEqual(result, {limit, workers:[limit, limit], mainObservedPreRun:true,
    lateRejected:true, lateJsRejected:true});
  writeFileSync(resultPath, JSON.stringify({passed:true, preRuns, ...result}, null, 2) + '\n');
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  clearTimeout(timer);
}
`;
// Match the production threading, Asyncify, ES-module, allocator, table and
// filesystem flags. A 64 MiB heap and EXIT_RUNTIME=1 bound this small fixture;
// it does not measure the production emulator's 3000 MiB memory reservation.
const flags = ['-O3', '-g', '-pthread', '-matomics', '-mbulk-memory',
  '-sASYNCIFY=1', '-sPROXY_TO_PTHREAD=1', '-sFORCE_FILESYSTEM', '-sALLOW_TABLE_GROWTH',
  '-sTOTAL_MEMORY=67108864', '-sWASM_BIGINT', '-sMALLOC=emmalloc', '-sEXPORT_ES6=1',
  '-sASYNCIFY_IMPORTS=ffi_call_js', '-sEXPORTED_RUNTIME_METHODS=addFunction,removeFunction,TTY,FS',
  '-sEXIT_RUNTIME=1', '-Wno-unused-function', '-Wno-sign-compare'];
const variants = [
  ['candidate', region, null],
  ['private-state', region.replace('static int instance_limit_state = MAX_INSTANCE_ALIVE;',
    'static __thread int instance_limit_state = MAX_INSTANCE_ALIVE;'), 'main did not observe preRun instance limit'],
  ['missing-export', region.replace('EMSCRIPTEN_KEEPALIVE\n', ''), 'missing qemu_wasm_set_instance_limit export'],
];
assert.notEqual(variants[1][1], region);
assert.notEqual(variants[2][1], region);
try {
  writeFileSync(join(directory, 'compiler-version.txt'), version.stdout);
  writeFileSync(join(directory, 'runner.mjs'), runner);
  for (const [name, code, failure] of variants) {
    const cPath = join(directory, `${name}.c`);
    const jsPath = join(directory, `${name}.mjs`);
    writeFileSync(cPath, prefix + code + suffix);
    const compile = spawnSync(compiler, [...flags, cPath, '-o', jsPath], {encoding:'utf8', timeout:120_000});
    writeFileSync(join(directory, `${name}-compiler.log`), (compile.stdout ?? '') + (compile.stderr ?? ''));
    assert.ifError(compile.error);
    assert.equal(compile.status, 0, compile.stderr);
    for (const limit of failure ? [8192] : [8192, 15000]) {
      const resultPath = join(directory, `${name}-${limit}.json`);
      const run = spawnSync(process.execPath, [join(directory, 'runner.mjs'), jsPath, String(limit), resultPath],
        {encoding:'utf8', timeout:20_000});
      const output = (run.stdout ?? '') + (run.stderr ?? '');
      writeFileSync(join(directory, `${name}-${limit}.log`), output);
      assert.ifError(run.error);
      if (failure) {
        assert.notEqual(run.status, 0, `${name} unexpectedly succeeded`);
        assert.ok(output.includes(failure), output);
        console.log(`PASS compiled bridge negative control rejects ${name}`);
      } else {
        assert.equal(run.status, 0, output);
        assert.equal(JSON.parse(readFileSync(resultPath, 'utf8')).passed, true);
        console.log(`PASS compiled SDK 4.0.10 preRun/shared main/two-pthread bridge at ${limit}`);
      }
    }
  }
  const files = Object.fromEntries(readdirSync(directory).sort().map(name => {
    const bytes = readFileSync(join(directory, name));
    return [name, {bytes:bytes.length, sha256:sha256(bytes)}];
  }));
  writeFileSync(join(directory, 'summary.json'), JSON.stringify({
    kind:'compiled_emscripten_budget_bridge', emscripten_version:'4.0.10',
    source_blob:blob(source), region_sha256:sha256(region),
    checker_sha256:sha256(readFileSync(fileURLToPath(import.meta.url))),
    flags, candidate:{total:2, passed:2}, negative_controls:{total:2, rejected:2}, files,
    scope:'Generated Emscripten glue with extracted C; excludes complete QEMU, browser GC, and browser acceptance',
  }, null, 2) + '\n');
  console.log('PASS compiled bridge proof; complete QEMU/browser acceptance remains separate');
} finally {
  if (temporary) rmSync(directory, {recursive:true, force:true});
}
