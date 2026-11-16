#!/usr/bin/env node
// Exercise the exact idle loop and instance-accounting helpers. The SDK mode
// queues a real JS task on a pthread; its payload is a finalization test double,
// not an assertion that a browser will run its garbage collector on schedule.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {dirname, join, resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const args = process.argv.slice(2);
const emscripten = args.at(-1) === '--emscripten';
if (emscripten) args.pop();
assert.ok(args.length === 1 || args.length === 2,
  'usage: check-idle-gc.mjs QEMU_SOURCE [PROOF_DIR] [--emscripten]');
const sourceRoot = resolve(args[0]);
const temporary = args.length === 1;
const output = temporary ? mkdtempSync(join(tmpdir(), 'qemu-idle-gc-proof-')) : resolve(args[1]);
mkdirSync(output, {recursive:true});
assert.deepEqual(readdirSync(output), [], 'Use an empty proof directory');
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const blob = bytes => createHash('sha1').update(`blob ${Buffer.byteLength(bytes)}\0`).update(bytes).digest('hex');
const script = fileURLToPath(import.meta.url);
const patch = readFileSync(join(dirname(script), 'idle-gc-yield.patch'));
const preimages = {
  'tcg/wasm32.c':'e0b26bdc8d6d5b987061f9090ae0b62b76c34e16',
  'system/cpus.c':'a444a747f0164ee9a998ef4b25b49a2efc4607a8',
  'include/tcg/wasm32-gc.h':null,
};
const postimages = {
  'tcg/wasm32.c':'5b6be92da27d573577854f6badf7fbbb1082f83f',
  'system/cpus.c':'9252d9478b067a88a776a47ca746e097591bf695',
  'include/tcg/wasm32-gc.h':'c3b9c7b9e3c65191cba9cc0cca117f45965a6a58',
};
const sources = Object.fromEntries(Object.keys(postimages).map(name => {
  const value = readFileSync(join(sourceRoot, name), 'utf8');
  assert.equal(blob(value), postimages[name], `Wrong candidate source: ${name}`);
  return [name, value];
}));
const reverseRoot = mkdtempSync(join(tmpdir(), 'qemu-idle-gc-reverse-'));
const before = {};
try {
  for (const [name, value] of Object.entries(sources)) {
    mkdirSync(dirname(join(reverseRoot, name)), {recursive:true});
    writeFileSync(join(reverseRoot, name), value);
  }
  const reverse = spawnSync('git', ['apply', '--reverse', '--whitespace=error-all', '-'],
    {cwd:reverseRoot, input:patch, encoding:'utf8', timeout:10_000});
  assert.ifError(reverse.error);
  assert.equal(reverse.status, 0, reverse.stderr);
  for (const [name, expected] of Object.entries(preimages)) {
    if (expected === null) {
      assert.throws(() => readFileSync(join(reverseRoot, name)), {code:'ENOENT'});
    } else {
      before[name] = readFileSync(join(reverseRoot, name), 'utf8');
      assert.equal(blob(before[name]), expected, `Wrong reversed source: ${name}`);
    }
  }
} finally { rmSync(reverseRoot, {recursive:true, force:true}); }
function region(source, start, end) {
  assert.equal(source.split(start).length, 2, `Non-unique start: ${start}`);
  const at = source.indexOf(start), last = source.indexOf(end, at + start.length);
  assert.ok(last > at, `Missing end: ${end}`);
  return source.slice(at, last);
}
const backend = sources['tcg/wasm32.c'];
const budget = region(backend, 'int instance_alive_global = 0;', 'static void add_instance_running_local');
assert.equal(budget, region(before['tcg/wasm32.c'], 'int instance_alive_global = 0;', 'static void add_instance_running_local'));
const activeYield = region(backend, '#define MAX_EXEC_NUM 50000', '__thread struct wasmContext ctx');
assert.equal(activeYield, region(before['tcg/wasm32.c'], '#define MAX_EXEC_NUM 50000', '__thread struct wasmContext ctx'));
assert.equal(backend.slice(backend.indexOf('uintptr_t QEMU_DISABLE_CFI tcg_qemu_tb_exec(')),
  before['tcg/wasm32.c'].slice(before['tcg/wasm32.c'].indexOf('uintptr_t QEMU_DISABLE_CFI tcg_qemu_tb_exec(')));
const helpers = region(backend, 'bool wasm32_has_pending_gc(void)', '#define MAX_EXEC_NUM 50000');
const idle = region(sources['system/cpus.c'], 'void qemu_wait_io_event(CPUState *cpu)', 'void cpus_kick_thread');
const oldIdle = region(before['system/cpus.c'], 'void qemu_wait_io_event(CPUState *cpu)', 'void cpus_kick_thread');
const poll = sources['system/cpus.c'].match(/^#define WASM_IDLE_GC_POLL_MS (\d+)$/m);
assert.equal(poll?.[1], '20');

const prefix = String.raw`
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#ifdef __EMSCRIPTEN__
#include <emscripten.h>
#else
#define EMSCRIPTEN_KEEPALIVE
#endif
#define __thread _Thread_local
#define EMSCRIPTEN 1
#define CONFIG_TCG 1
#define WASM_IDLE_GC_POLL_MS 20
#define qatomic_read(p) __atomic_load_n(p, __ATOMIC_RELAXED)
#define qatomic_sub(p, n) ((void)__atomic_fetch_sub(p, n, __ATOMIC_SEQ_CST))
#define qatomic_cmpxchg(p, old, next) ({ int observed = (old); \
    __atomic_compare_exchange_n(p, &observed, next, false, \
                               __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST); observed; })
static void require(bool yes, const char *message) {
    if (!yes) { fprintf(stderr, "IDLE_GC_FAILURE: %s\n", message); abort(); }
}
static int load(const int *p) { return __atomic_load_n(p, __ATOMIC_SEQ_CST); }
static void store(int *p, int v) { __atomic_store_n(p, v, __ATOMIC_SEQ_CST); }
static void increment(int *p) { __atomic_fetch_add(p, 1, __ATOMIC_SEQ_CST); }
static __thread bool initdone;
static __thread bool idle_owner;
static void remove_module_js(void) { abort(); }
static void gc_test_sleep(unsigned milliseconds);
#define emscripten_sleep gc_test_sleep
`;
const fixture = String.raw`
#undef emscripten_sleep
typedef pthread_cond_t QemuCond;
typedef pthread_mutex_t QemuMutex;
typedef struct CPUState { QemuCond *halt_cond; bool idle; } CPUState;
static QemuMutex qemu_global_mutex = PTHREAD_MUTEX_INITIALIZER;
static QemuCond halt_cond = PTHREAD_COND_INITIALIZER;
static CPUState cpu = {.halt_cond=&halt_cond, .idle=true};
static int ready, allow_delivery, delivered, callback_bad_lock, owner_bql_held;
static int timed_waits, normal_waits, yields, inside_yield, kick_sent;
static int idle_callbacks, resume_callbacks, common_calls;
static int scenario;
/* Cases: queued finalization, no pending, already delivered, kick in wait,
 * kick during yield, and uninitialized non-JIT CPU. */
enum { QUEUED, NO_PENDING, DELIVERED, KICK_WAIT, KICK_YIELD, UNINITIALIZED };
static double host_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000.0 + t.tv_nsec / 1000000.0;
}
static void host_pause(void) {
#ifdef __EMSCRIPTEN__
    emscripten_sleep(1);
#else
    struct timespec t = {.tv_nsec=1000000};
    nanosleep(&t, NULL);
#endif
}
static void await_value(const int *p, const char *message) {
    double deadline = host_ms() + 2000;
    while (!load(p) && host_ms() < deadline) host_pause();
    require(load(p) != 0, message);
}
static void qemu_mutex_lock_iothread(void) {
    require(pthread_mutex_lock(&qemu_global_mutex) == 0, "BQL lock failed");
    if (idle_owner) store(&owner_bql_held, 1);
}
static void qemu_mutex_unlock_iothread(void) {
    if (idle_owner) store(&owner_bql_held, 0);
    require(pthread_mutex_unlock(&qemu_global_mutex) == 0, "BQL unlock failed");
}
static bool cpu_thread_is_idle(CPUState *c) {
    require(load(&owner_bql_held), "idle predicate evaluated without BQL");
    return c->idle;
}
static void qemu_cond_wait(QemuCond *cond, QemuMutex *mutex) {
    increment(&normal_waits);
    store(&owner_bql_held, 0);
    require(pthread_cond_wait(cond, mutex) == 0, "condition wait failed");
    store(&owner_bql_held, 1);
}
static void qemu_cond_timedwait(QemuCond *cond, QemuMutex *mutex, int ms) {
    require(ms == 20, "changed idle polling interval");
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_nsec += ms * 1000000;
    if (deadline.tv_nsec >= 1000000000) { deadline.tv_nsec -= 1000000000; deadline.tv_sec++; }
    increment(&timed_waits);
    store(&owner_bql_held, 0);
    /* Only this kick-ordering case holds the adapter wait until the kick.
     * It must not depend on the controller being scheduled within 20 ms.
     * QUEUED exercises the real timed wait and its periodic wakeups. */
    int status = scenario == KICK_WAIT ? pthread_cond_wait(cond, mutex) :
        pthread_cond_timedwait(cond, mutex, &deadline);
    store(&owner_bql_held, 1);
    require(status == 0 || status == ETIMEDOUT, "timed condition wait failed");
}
static void qemu_plugin_vcpu_idle_cb(CPUState *c) { (void)c; idle_callbacks++; }
static void qemu_plugin_vcpu_resume_cb(CPUState *c) { (void)c; resume_callbacks++; }
static void qemu_wait_io_event_common(CPUState *c) {
    (void)c; require(load(&owner_bql_held), "common work lost BQL"); common_calls++;
}
static void kick(void) {
    qemu_mutex_lock_iothread();
    cpu.idle = false;
    require(pthread_cond_broadcast(&halt_cond) == 0, "kick failed");
    qemu_mutex_unlock_iothread();
    store(&kick_sent, 1);
}
static void gc_test_sleep(unsigned milliseconds) {
    require(milliseconds == 0, "GC opportunity must use zero sleep");
    require(!load(&owner_bql_held), "yield kept BQL");
    increment(&yields);
    store(&inside_yield, 1);
#ifdef __EMSCRIPTEN__
    emscripten_sleep(0);
#else
    if (scenario == KICK_YIELD) await_value(&kick_sent, "kick did not arrive during yield");
    if (load(&allow_delivery) && !load(&delivered)) {
        instance_garbage_collected_local = 1;
        store(&delivered, 1);
    }
#endif
    store(&inside_yield, 0);
}
`;
const exercise = String.raw`
static void *idle_worker(void *unused) {
    (void)unused;
    idle_owner = true;
    initdone = scenario != UNINITIALIZED;
    instance_alive_local = scenario == NO_PENDING ? 1 : 2;
    instance_running_local = 1;
    if (scenario == DELIVERED) {
        instance_garbage_collected_local = 1;
        store(&delivered, 1);
    }
#ifdef __EMSCRIPTEN__
    if (scenario == QUEUED) {
        EM_ASM({
            const callbackCount = $0 >> 2, delivered = $1 >> 2;
            const allow = $2 >> 2, bql = $3 >> 2, badLock = $4 >> 2;
            function deliver() {
                if (!Atomics.load(HEAP32, allow)) { setTimeout(deliver, 1); return; }
                if (Atomics.load(HEAP32, bql)) Atomics.store(HEAP32, badLock, 1);
                Atomics.store(HEAP32, callbackCount, 1);
                Atomics.store(HEAP32, delivered, 1);
            }
            setTimeout(deliver, 0);
        }, &instance_garbage_collected_local, &delivered, &allow_delivery,
           &owner_bql_held, &callback_bad_lock);
    }
#endif
    qemu_mutex_lock_iothread();
    store(&ready, 1);
    qemu_wait_io_event(&cpu);
    require(!cpu.idle, "returned without rechecking idle after kick");
    qemu_mutex_unlock_iothread();
    /* The baseline can only service its already queued JS task after the
     * external kick returns it from the indefinite condition wait. */
    if (EXPECT_BASELINE && scenario == QUEUED) {
        double deadline = host_ms() + 2000;
        while (!load(&delivered) && host_ms() < deadline) gc_test_sleep(0);
        require(load(&delivered), "baseline callback missing even after kick");
        check_instance_garbage_collected();
    }
    return NULL;
}
int main(int argc, char **argv) {
    require(argc == 2, "missing scenario");
    scenario = atoi(argv[1]);
    require(scenario >= QUEUED && scenario <= UNINITIALIZED, "bad scenario");
    instance_limit_state = -MAX_INSTANCE_ALIVE;
    instance_alive_global = MAX_INSTANCE_ALIVE;
    pthread_t worker;
    require(pthread_create(&worker, NULL, idle_worker, NULL) == 0, "pthread_create failed");
    await_value(&ready, "idle worker did not start");
    bool progress_before_kick = false;
    if (scenario == QUEUED) {
        require(!load(&delivered) && !try_reserve_instance(), "credit released before queued callback");
        store(&allow_delivery, 1);
        if (EXPECT_BASELINE) {
            double until = host_ms() + 100;
            while (host_ms() < until) host_pause();
            require(!load(&delivered) && !try_reserve_instance(), "baseline unexpectedly serviced queued callback");
            require(load(&normal_waits) > 0 && load(&timed_waits) == 0, "baseline did not block normally");
        } else {
            double until = host_ms() + 2000;
            while (!can_add_instance() && host_ms() < until) host_pause();
            require(load(&delivered) && can_add_instance(), "idle worker did not return delivered credit");
            require(try_reserve_instance() && !can_add_instance(), "active worker could not use exactly one returned credit");
            progress_before_kick = true;
        }
    } else if (scenario == KICK_YIELD) {
        await_value(&inside_yield, "idle worker did not reach unlocked yield");
    } else if (scenario == KICK_WAIT) {
        await_value(&timed_waits, "idle worker did not reach timed wait");
    } else {
        await_value(&normal_waits, "no-pending path did not retain normal wait");
        require(load(&timed_waits) == 0 && load(&yields) == 0, "no-pending path changed scheduling");
        if (scenario == DELIVERED) {
            require(try_reserve_instance() && !can_add_instance(), "already delivered callback was not consumed before wait");
        } else {
            require(!try_reserve_instance(), "no callback released budget");
        }
    }
    kick();
    require(pthread_join(worker, NULL) == 0, "pthread_join failed");
    require(idle_callbacks == 1 && resume_callbacks == 1 && common_calls == 1, "idle/resume/common semantics changed");
    require(!load(&callback_bad_lock), "JS callback ran with BQL held");
    if (scenario == KICK_WAIT) require(load(&yields) == 0, "kick during wait incurred unnecessary GC yield");
    if (scenario == KICK_YIELD) require(load(&yields) == 1, "kick during yield was lost");
    if (EXPECT_BASELINE && scenario == QUEUED)
        require(try_reserve_instance() && !can_add_instance(), "baseline callback credit missing after external kick");
    printf("IDLE_GC_RESULT: {\"scenario\":%d,\"baseline\":%s,\"progressBeforeKick\":%s,\"timedWaits\":%d,\"normalWaits\":%d,\"yields\":%d,\"delivered\":%d}\n",
        scenario, EXPECT_BASELINE ? "true" : "false", progress_before_kick ? "true" : "false",
        load(&timed_waits), load(&normal_waits), load(&yields), load(&delivered));
    return 0;
}
`;
const runner = String.raw`
import assert from 'node:assert/strict';
import {writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [binary, scenario, output] = process.argv.slice(2);
let report, timer, resolveExit, rejectExit;
const exited = new Promise((resolve, reject) => { resolveExit=resolve; rejectExit=reject; });
try {
  const {default: initialize} = await import(pathToFileURL(resolve(binary)).href);
  const initialized = initialize({arguments:[scenario],
    print(line) {
      console.log(line);
      if (line.startsWith('IDLE_GC_RESULT: ')) report=JSON.parse(line.slice(16));
    }, printErr:line=>console.error(line),
    onAbort:reason=>rejectExit(new Error(String(reason))),
    onExit:code=>code===0 ? resolveExit() : rejectExit(new Error('exit '+code))});
  await Promise.race([Promise.all([initialized,exited]),
    new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error('idle bridge timeout')),10000);})]);
  assert.equal(report?.scenario,Number(scenario));
  writeFileSync(output,JSON.stringify(report,null,2)+'\n');
} catch(error) { console.error(error); process.exitCode=1; }
finally { clearTimeout(timer); }
`;
const compiler = emscripten ? (process.env.BROWSER_QEMU_EMCC ?? 'emcc') : (process.env.BROWSER_QEMU_NATIVE_CC ?? 'cc');
const version = spawnSync(compiler, ['--version'], {encoding:'utf8', timeout:30_000});
assert.ifError(version.error);
assert.equal(version.status, 0, version.stderr);
if (emscripten) assert.match(version.stdout, /\b4\.0\.10\b/, 'Use the production SDK version');
const flags = emscripten ? ['-O3','-g','-pthread','-matomics','-mbulk-memory',
  '-sASYNCIFY=1','-sPROXY_TO_PTHREAD=1','-sFORCE_FILESYSTEM','-sALLOW_TABLE_GROWTH',
  '-sTOTAL_MEMORY=67108864','-sWASM_BIGINT','-sMALLOC=emmalloc','-sEXPORT_ES6=1',
  '-sASYNCIFY_IMPORTS=ffi_call_js','-sEXPORTED_RUNTIME_METHODS=addFunction,removeFunction,TTY,FS',
  '-sEXIT_RUNTIME=1','-Wno-unused-function','-Wno-sign-compare'] :
  ['-std=gnu11','-Wall','-Wextra','-Werror','-Wno-unused-function','-Wno-sign-compare','-pthread'];
const scenarios = emscripten ? [0,1,2,5] : [0,1,2,3,4,5];
const unsafeIdle = idle.replace('                qemu_mutex_unlock_iothread();\n','');
assert.notEqual(unsafeIdle, idle);
const variants = [['candidate',idle,false,null],['baseline',oldIdle,true,null],
  ['locked-yield',unsafeIdle,false,'yield kept BQL']];
const results = [];
try {
  writeFileSync(join(output,'compiler-version.txt'),version.stdout);
  writeFileSync(join(output,'runner.mjs'),runner);
  writeFileSync(join(output,'idle-gc-yield.patch'),patch);
  for (const [name,value] of Object.entries(sources)) writeFileSync(join(output,'candidate-'+name.replaceAll('/','_')),value);
  for (const [name,value] of Object.entries(before)) writeFileSync(join(output,'baseline-'+name.replaceAll('/','_')),value);
  for (const [name,loop,baseline,failure] of variants) {
    const cPath=join(output,`${name}.c`), binary=join(output,emscripten ? `${name}.mjs` : name);
    writeFileSync(cPath,`#define EXPECT_BASELINE ${baseline ? 1 : 0}\n`+prefix+budget+helpers+fixture+loop+exercise);
    const compiled=spawnSync(compiler,[...flags,cPath,'-o',binary],{encoding:'utf8',timeout:120_000});
    writeFileSync(join(output,`${name}-compiler.log`),(compiled.stdout??'')+(compiled.stderr??''));
    assert.ifError(compiled.error);
    assert.equal(compiled.status,0,compiled.stderr);
    for (const scenario of name==='candidate' ? scenarios : [0]) {
      const resultPath=join(output,`${name}-${scenario}.json`);
      const run=emscripten ? spawnSync(process.execPath,[join(output,'runner.mjs'),binary,String(scenario),resultPath],
        {encoding:'utf8',timeout:15_000}) : spawnSync('bash',['-c','ulimit -c 0\nexec "$1" "$2"','idle-gc-test',binary,String(scenario)],
        {encoding:'utf8',timeout:10_000});
      const log=(run.stdout??'')+(run.stderr??'');
      writeFileSync(join(output,`${name}-${scenario}.log`),log);
      assert.ifError(run.error);
      if (failure) {
        assert.notEqual(run.status,0,`${name} unexpectedly succeeded`);
        assert.ok(log.includes(failure),log);
        results.push({name,scenario,rejected:true});
      } else {
        assert.equal(run.status,0,log);
        const line=log.split('\n').find(line=>line.startsWith('IDLE_GC_RESULT: '));
        assert.ok(line,log);
        const report=JSON.parse(line.slice('IDLE_GC_RESULT: '.length));
        if (scenario===0) assert.equal(report.progressBeforeKick,!baseline);
        writeFileSync(resultPath,JSON.stringify(report,null,2)+'\n');
        results.push({name,scenario,passed:true,...report});
      }
      console.log(`PASS ${emscripten ? 'SDK JS queue' : 'native locks'} ${name} scenario ${scenario}`);
    }
  }
  const files=Object.fromEntries(readdirSync(output).sort().map(name=>{
    const bytes=readFileSync(join(output,name));return [name,{bytes:bytes.length,sha256:sha256(bytes)}];
  }));
  writeFileSync(join(output,'summary.json'),JSON.stringify({
    kind:emscripten ? 'compiled_emscripten_idle_gc_queue' : 'compiled_native_idle_gc_locking',
    status:'passed',qemu_commit:'8604ed49a3cde392890b014a8d5a959c8a2fe72a',
    emscripten_version:emscripten ? '4.0.10' : null,preimages,postimages,
    policy:'pending_local_finalizers_timed_idle_wait',poll_ms:20,patch_apply_reverse:true,
    unchanged_budget_region:true,unchanged_active_dispatch:true,unchanged_active_yield:true,
    patch_sha256:sha256(patch),checker_sha256:sha256(readFileSync(script)),flags,results,files,
    scope:'Exact source fragments with QEMU lock/predicate adapters; queued finalization is a deterministic test double. No real GC scheduling, full QEMU or browser acceptance proof.',
  },null,2)+'\n');
} finally { if(temporary) rmSync(output,{recursive:true,force:true}); }
