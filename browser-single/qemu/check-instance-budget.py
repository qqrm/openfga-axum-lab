#!/usr/bin/env python3
"""Exercise extracted QEMU budget functions; no browser, VM, or model copy."""
import hashlib
import os
from pathlib import Path
import re
import resource
import signal
import subprocess
import sys
import tempfile

if len(sys.argv) != 2:
    raise SystemExit(f"usage: {sys.argv[0]} /path/to/qemu/tcg/wasm32.c")
source = Path(sys.argv[1]).read_text()
start, end = "int instance_alive_global = 0;", "static void add_instance_running_local"
assert source.count(start) == source.count(end) == 1, "Unexpected source boundaries"
region = source[source.index(start):source.index(end)]
guard = "if (instance_running_local < MAX_INSTANCE_RUNNING_LOCAL)"
assert region.count(guard) == 1, "Missing retained-instance guard"

PREFIX = r'''
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
'''
SUFFIX = r'''
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
'''
old_guard = region.replace(guard, "if (instance_pending_gc_local() > 0)")
racy_reserve, replacements = re.subn(
    r"static bool try_reserve_instance\((?:void)?\)\n\{.*?\n\}",
    "static bool try_reserve_instance(void)\n{\n"
    "    check_instance_garbage_collected();\n"
    "    if (!can_add_instance()) return false;\n"
    "    qatomic_inc(&instance_alive_global);\n    return true;\n}", region, flags=re.S)
assert replacements == 1, "Unexpected reservation function"
resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
with tempfile.TemporaryDirectory(prefix="qemu-budget-test-") as directory:
    for name, code, failure in [
        ("candidate", region, None),
        ("old-pending-guard", old_guard, "instance_running_local == 1024"),
        ("non-atomic-admission", racy_reserve, "winners == 1"),
    ]:
        path = Path(directory) / f"{name}.c"
        binary = path.with_suffix("")
        path.write_text(PREFIX + code + SUFFIX)
        subprocess.run([os.environ.get("BROWSER_QEMU_NATIVE_CC", "cc"), "-std=gnu11", "-Wall", "-Wextra",
                        "-Werror", "-Wno-unused-function", "-Wno-sign-compare", "-pthread", str(path),
                        "-o", str(binary)], check=True, timeout=30)
        result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
        if failure is None:
            assert result.returncode == 0, result.stderr
            print(result.stdout, end="")
        else:
            assert result.returncode == -signal.SIGABRT and failure in result.stderr, result
            print(f"PASS negative control rejects {name}")
print("PASS extracted-region SHA256", hashlib.sha256(region.encode()).hexdigest())
print("Native budget checks exclude JS GC timing, pointer-width TB lookup, and browser acceptance.")
