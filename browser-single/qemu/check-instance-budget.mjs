#!/usr/bin/env node
// Exercise extracted QEMU budget functions; no browser, VM, or model copy.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

assert.equal(process.argv.length, 3, `usage: ${process.argv[1]} /path/to/qemu/tcg/wasm32.c`);
const source = readFileSync(process.argv[2], 'utf8');
const start = 'int instance_alive_global = 0;';
const end = 'static void add_instance_running_local';
assert.equal(source.split(start).length, 2, 'Unexpected source start');
assert.equal(source.split(end).length, 2, 'Unexpected source end');
const region = source.slice(source.indexOf(start), source.indexOf(end));
const guard = 'if (instance_running_local < MAX_INSTANCE_RUNNING_LOCAL)';
assert.equal(region.split(guard).length, 2, 'Missing retained-instance guard');

const prefix = String.raw`
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
/* C11 spelling also accepts upstream's "__thread static" declaration order. */
#define __thread _Thread_local
static pthread_barrier_t admission_gate;
static __thread bool synchronize_read;
/* Int-only native adapter for pinned qatomic.h: relaxed load, strong seq-cst
 * CAS returning the observed old value, and seq-cst subtraction/increment.
 * The barrier forces both contenders to observe cap-1 before either CAS. */
static int budget_read(const int *p) {
    int value = __atomic_load_n(p, __ATOMIC_RELAXED);
    if (synchronize_read) {
        synchronize_read = false;
        int status = pthread_barrier_wait(&admission_gate);
        assert(status == 0 || status == PTHREAD_BARRIER_SERIAL_THREAD);
    }
    return value;
}
#define qatomic_read(p) budget_read(p)
#define qatomic_sub(p, n) ((void)__atomic_fetch_sub(p, n, __ATOMIC_SEQ_CST))
#define qatomic_inc(p) ((void)__atomic_fetch_add(p, 1, __ATOMIC_SEQ_CST))
#define qatomic_cmpxchg(p, old, next) ({ int observed = (old); \
    __atomic_compare_exchange_n(p, &observed, next, false, \
                               __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST); observed; })
static void remove_module_js(void);
`;
const suffix = String.raw`
static int detached;
static void remove_module_js(void) {
    assert(to_remove_instance_idx == 1024);
    for (int i = 0; i < to_remove_instance_idx; i++)
        assert(to_remove_instance[i] == detached + i + 1);
    detached += to_remove_instance_idx;
    to_remove_instance_idx = 0;
}
static void fixture(int retained, int alive, int begin) {
    instance_alive_global = instance_alive_local = alive;
    instance_running_local = retained;
    instance_garbage_collected_local = 0;
    instance_running_begin = begin;
    instance_running_end = (begin + retained) % INSTANCE_RUNNING_LEN;
    to_remove_instance_idx = detached = 0;
    memset(instance_running, 0, sizeof(instance_running));
    for (int i = 0; i < retained; i++) {
        int slot = (begin + i) % INSTANCE_RUNNING_LEN;
        instance_running[slot].tb = (uint8_t *)(uintptr_t)(i + 1);
        instance_running[slot].fidx = i + 1;
    }
}
static void *contend(void *result) {
    synchronize_read = true;
    *(bool *)result = try_reserve_instance();
    return NULL;
}
int main(void) {
    assert(MAX_INSTANCE_ALIVE == 8192 && MAX_INSTANCE_RUNNING_LOCAL == 2048);
    fixture(2048, 4096, 0); /* One earlier batch still awaits callbacks. */
    remove_instance_running_local();
    assert(instance_running_local == 1024 && detached == 1024);
    assert(instance_alive_local == 4096 && instance_alive_global == 4096);
    assert(instance_pending_gc_local() == 3072);
    puts("PASS early FIFO eviction while previous GC is pending");
    for (int i = 0; i < 8; i++) remove_instance_running_local();
    assert(instance_running_local == 1024 && detached == 1024);
    puts("PASS repeated blocked calls preserve the retained working set");
    instance_garbage_collected_local = 1024;
    check_instance_garbage_collected();
    assert(instance_alive_local == 3072 && instance_alive_global == 3072);
    assert(instance_garbage_collected_local == 0);
    instance_garbage_collected_local = 512;
    assert(try_reserve_instance()); /* Consume callbacks before admission. */
    assert(instance_alive_global == 2561 && instance_alive_local == 2560);
    inc_instance_local(); /* Actual local accounting commit, without TB casts. */
    assert(instance_alive_local == 2561 && instance_running_local == 1025);
    puts("PASS only processed callbacks release alive slots; reservation is counted");
    int begin = INSTANCE_RUNNING_LEN - 512;
    fixture(2048, 2048, begin);
    remove_instance_running_local();
    assert(instance_running_begin == 512 && instance_running_end == 1536);
    for (int i = 0; i < 2048; i++) {
        uint8_t *actual = instance_running[(begin + i) % INSTANCE_RUNNING_LEN].tb;
        assert(actual == (i < 1024 ? NULL : (uint8_t *)(uintptr_t)(i + 1)));
    }
    puts("PASS wrapped FIFO removes precisely the oldest half");
    fixture(0, 0, 0);
    instance_alive_global = MAX_INSTANCE_ALIVE - 1;
    bool results[2] = {false, false};
    pthread_t threads[2];
    assert(pthread_barrier_init(&admission_gate, NULL, 2) == 0);
    for (int i = 0; i < 2; i++)
        assert(pthread_create(&threads[i], NULL, contend, &results[i]) == 0);
    for (int i = 0; i < 2; i++) assert(pthread_join(threads[i], NULL) == 0);
    int winners = results[0] + results[1];
    assert(winners == 1);
    assert(instance_alive_global == MAX_INSTANCE_ALIVE);
    assert(!can_add_instance() && !try_reserve_instance());
    assert(pthread_barrier_destroy(&admission_gate) == 0);
    puts("PASS concurrent admission at 8191 reserves exactly one final slot");
}
`;

const oldGuard = region.replace(guard, 'if (instance_pending_gc_local() > 0)');
const reservePattern = /static bool try_reserve_instance\((?:void)?\)\n\{.*?\n\}/gs;
assert.equal([...region.matchAll(reservePattern)].length, 1, 'Unexpected reservation function');
const racyReserve = region.replace(reservePattern, String.raw`static bool try_reserve_instance(void)
{
    check_instance_garbage_collected();
    if (!can_add_instance()) return false;
    qatomic_inc(&instance_alive_global);
    return true;
}`);
const directory = mkdtempSync(join(tmpdir(), 'qemu-budget-test-'));
try {
  for (const [name, code, failure] of [
    ['candidate', region, null],
    ['old-pending-guard', oldGuard, 'instance_running_local == 1024'],
    ['non-atomic-admission', racyReserve, 'winners == 1'],
  ]) {
    const path = join(directory, `${name}.c`);
    const binary = join(directory, name);
    writeFileSync(path, prefix + code + suffix);
    const compiled = spawnSync(process.env.BROWSER_QEMU_NATIVE_CC ?? 'cc', [
      '-std=gnu11', '-Wall', '-Wextra', '-Werror', '-Wno-unused-function',
      '-Wno-sign-compare', '-pthread', path, '-o', binary,
    ], { encoding: 'utf8', timeout: 30_000 });
    assert.ifError(compiled.error);
    assert.equal(compiled.status, 0, compiled.stderr);
    const result = spawnSync('bash', ['-c', 'ulimit -c 0\nexec "$1"', 'budget-test', binary],
      { encoding: 'utf8', timeout: 10_000 });
    assert.ifError(result.error);
    if (failure === null) {
      assert.equal(result.status, 0, result.stderr);
      process.stdout.write(result.stdout);
    } else {
      assert.equal(result.signal, 'SIGABRT', result.stderr);
      assert.ok(result.stderr.includes(failure), result.stderr);
      console.log(`PASS negative control rejects ${name}`);
    }
  }
} finally {
  rmSync(directory, { recursive: true, force: true });
}
console.log('PASS extracted-region SHA256', createHash('sha256').update(region).digest('hex'));
console.log('Native budget checks exclude JS GC timing, pointer-width TB lookup, and browser acceptance.');
