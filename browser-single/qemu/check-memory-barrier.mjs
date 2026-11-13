#!/usr/bin/env node
// Source-derived, bounded proof of the missing compiled QEMU Wasm fence.
// No guest image, full QEMU build, full VM, CI, or performance claim.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const base = dirname(fileURLToPath(import.meta.url));
const [target, output, ...extra] = process.argv.slice(2);
assert.ok(target && extra.length === 0,
  'usage: node check-memory-barrier.mjs PATCHED_TCG_TARGET_C [EMPTY_PROOF_DIR]');
const sourceRoot = resolve(dirname(target), '../..');
assert.equal(resolve(target), join(sourceRoot, 'tcg', 'wasm32', 'tcg-target.c.inc'));
const temp = mkdtempSync(join(tmpdir(), 'qemu-memory-barrier-'));
process.on('exit', () => rmSync(temp, { recursive: true, force: true }));
const proof = output ? resolve(output) : join(temp, 'proof');
mkdirSync(proof, { recursive: true });
assert.equal(readdirSync(proof).length, 0, 'proof directory must be empty');
const hash = (algorithm, data) => createHash(algorithm).update(data).digest('hex');
const blob = data => hash('sha1', Buffer.concat([Buffer.from('blob '+Buffer.byteLength(data)+'\0'), Buffer.from(data)]));
const pins = {
  'baseline-tcg-target.c.inc': 'd4078f5adac57d9ccc7d792773fff2ea65a33d6a',
  'patched-tcg-target.c.inc': '4b3e9f8fabcaa3b9a2bb33746f7c1d7046691355',
  'tcg-target.h': '37ad4b93c8ab721639e4cd8603a5dbbe69c07105',
  'wasm32.c': 'e0b26bdc8d6d5b987061f9090ae0b62b76c34e16',
  'tcg-mo.h': 'c2c55704e1375a88fc7b53ee4c113cf639b98d67',
  'tcg-op.c': 'aa6bc6f57dd13f2d7aaed6262edebf58b2061873',
  'tcg-op-ldst.c': 'f11043b4498ec1c79679236fa7b94295a88386da',
  'cpu.h': 'ef987f344cff2409b7746b12f654ed7e3fb560dd',
  'atomic.h': 'ec752d6e151b0b2ae8a5227e865b7df653e10115',
};
const sources = {};
const sourcePaths = {
  'tcg-target.h': 'tcg/wasm32/tcg-target.h',
  'wasm32.c': 'tcg/wasm32.c',
  'tcg-mo.h': 'include/tcg/tcg-mo.h',
  'tcg-op.c': 'tcg/tcg-op.c',
  'tcg-op-ldst.c': 'tcg/tcg-op-ldst.c',
  'cpu.h': 'target/i386/cpu.h',
  'atomic.h': 'include/qemu/atomic.h',
};
sources['patched-tcg-target.c.inc'] = readFileSync(target, 'utf8');
assert.equal(blob(sources['patched-tcg-target.c.inc']), pins['patched-tcg-target.c.inc']);
sources['baseline-tcg-target.c.inc'] = sources['patched-tcg-target.c.inc'].replace(
  mbCase(sources['patched-tcg-target.c.inc']),
  '    case INDEX_op_mb:\n        tcg_tci_out_op_v(s, opc);\n        tcg_wasm_out8(s, 0x01); // nop\n        break;');
for (const [name, pin] of Object.entries(pins)) {
  const data = sources[name] ?? readFileSync(join(sourceRoot, sourcePaths[name]), 'utf8');
  assert.equal(blob(data), pin, name+' exact full source');
  sources[name] = data;
  writeFileSync(join(proof, name), data);
}
const patch = readFileSync(join(base, 'memory-barrier.patch'), 'utf8');
assert.equal(hash('sha256', patch), '49e7301bd139c641a329ac0c433f6f6fe85dcd155e712bc159ee1818d538e339');
writeFileSync(join(proof, 'memory-barrier.patch'), patch);
writeFileSync(join(proof, 'check-memory-barrier.mjs'), readFileSync(fileURLToPath(import.meta.url)));

function extract(source, name) {
  const pattern = new RegExp('^(?:static(?: inline)? )?[^\\n]*\\b'+name+'\\([^)]*\\)\\s*\\{', 'gm');
  const matches = [...source.matchAll(pattern)];
  assert.equal(matches.length, 1, 'one actual C function: '+name);
  const start = matches[0].index;
  let cursor = start + matches[0][0].length, depth = 1;
  // Selected pinned helpers contain no braces in strings or comments.
  while (depth && cursor < source.length) {
    const char = source[cursor++];
    if (char === '{') depth++;
    if (char === '}') depth--;
  }
  assert.equal(depth, 0);
  return source.slice(start, cursor);
}
function mbCase(source) {
  const matches = [...source.matchAll(/^    case INDEX_op_mb:\n[\s\S]*?^        break;/gm)];
  assert.equal(matches.length, 1);
  return matches[0][0];
}
const baseline = sources['baseline-tcg-target.c.inc'];
const candidate = sources['patched-tcg-target.c.inc'];
// The baseline includes the already verified eight-call i32 multiply repair.
// Reverse only that repair to pin its upstream parent as well as both postimages.
let originalUpstream = baseline;
for (const [name, extension] of [['mulu2_i32', 'u'], ['muls2_i32', 's']]) {
  const fixed = extract(baseline, 'tcg_wasm_out_'+name);
  let original = fixed;
  for (const operand of ['arg1', 'arg2']) {
    const instruction = '    tcg_wasm_out_op_global_get_r(s, '+operand+');';
    const normalized = instruction+'\n    tcg_wasm_out_op_i32_wrap_i64(s);'+
      '\n    tcg_wasm_out_op_i64_extend_i32_'+extension+'(s);';
    assert.equal(original.split(normalized).length, 2);
    original = original.replace(normalized, instruction);
  }
  originalUpstream = originalUpstream.replace(fixed, original);
}
assert.equal(blob(originalUpstream), '3c85588be97b6c53a451e30c0593e8b433ede7b6');
writeFileSync(join(proof, 'upstream-tcg-target.c.inc'), originalUpstream);
assert.equal(candidate.replace(mbCase(candidate), mbCase(baseline)), baseline,
  'the complete production change is the single mb emitter case');
assert.ok(/case INDEX_op_mb:\s*\/\* Ensure ordering for all kinds \*\/\s*smp_mb\(\);\s*break;/.test(sources['wasm32.c']), 'actual TCI uses smp_mb');
assert.ok(/#define smp_mb\(\)\s+\(\{ barrier\(\); __atomic_thread_fence\(__ATOMIC_SEQ_CST\); \}\)/.test(sources['atomic.h']), 'actual smp_mb uses seq_cst');
assert.ok(sources['tcg-op-ldst.c'].includes('tcg_gen_req_mo(TCG_MO_LD_LD | TCG_MO_ST_LD);'), 'actual load-site request');
assert.ok(sources['tcg-op-ldst.c'].includes('tcg_gen_req_mo(TCG_MO_LD_ST | TCG_MO_ST_ST);'), 'actual store-site request');

function command(program, args, timeout = 30_000) {
  const result = spawnSync(program, args, { encoding: 'utf8', timeout });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr || result.stdout);
  return { program, args, status: result.status, stdout: result.stdout, stderr: result.stderr };
}
// Apply and reverse the exact patch on an isolated copy, preserving the i32 fix.
mkdirSync(join(temp, 'tcg', 'wasm32'), { recursive: true });
const patchTarget = join(temp, 'tcg', 'wasm32', 'tcg-target.c.inc');
writeFileSync(patchTarget, baseline);
const forward = command('git', ['-C', temp, 'apply', '--whitespace=error-all', join(base, 'memory-barrier.patch')]);
assert.equal(readFileSync(patchTarget, 'utf8'), candidate);
const reverse = command('git', ['-C', temp, 'apply', '--reverse', '--whitespace=error-all', join(base, 'memory-barrier.patch')]);
assert.equal(readFileSync(patchTarget, 'utf8'), baseline);
writeFileSync(join(proof, 'patch-apply-reverse.json'), JSON.stringify({ forward, reverse }, null, 2)+'\n');

const orderingEnum = sources['tcg-mo.h'].match(/typedef enum \{[\s\S]*?\} TCGBar;/)?.[0];
const hostMO = sources['tcg-target.h'].match(/^#define TCG_TARGET_DEFAULT_MO[^\n]*$/m)?.[0];
const guestMO = sources['cpu.h'].match(/^#define TCG_GUEST_DEFAULT_MO[^\n]*$/m)?.[0];
assert.ok(orderingEnum && hostMO && guestMO);
// The adapters only supply output buffers, an opcode tag, and the TCG IR recorder.
// The actual case, both byte/word emitters, MB generator, masks and req_mo stay verbatim.
const adapter = String.raw`
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
typedef unsigned TCGOpcode;
typedef uintptr_t TCGArg;
enum { INDEX_op_mb = 0xa5 }; /* A distinctive test tag, not a replacement TCG enum. */
typedef struct { uint8_t bytes[8]; size_t used; uint32_t words[2]; size_t nwords; } TCGContext;
static void tcg_sub_out8(TCGContext *s, uint32_t value) {
    assert(s->used < sizeof(s->bytes)); s->bytes[s->used++] = (uint8_t)value;
}
static void tcg_out32(TCGContext *s, uint32_t value) {
    assert(s->nwords < 2); s->words[s->nwords++] = value;
}
static struct { unsigned guest_mo; } context, *tcg_ctx = &context;
static unsigned generated, generated_type;
static void tcg_gen_op1(TCGOpcode op, unsigned arg) {
    assert(op == INDEX_op_mb); generated++; generated_type = arg;
}
`;
const driver = String.raw`
int main(void) {
    tcg_ctx->guest_mo = TCG_GUEST_DEFAULT_MO;
    for (unsigned request = 0; request < 16; request++) {
        generated = 0; generated_type = 0;
        tcg_gen_req_mo((TCGBar)request);
        printf("REQ %u %u %u\n", request, generated, generated_type);
    }
    for (unsigned mask = 0; mask < 64; mask++) {
        TCGContext s = {0}; TCGArg args[] = {mask};
        emit_mb(&s, INDEX_op_mb, args);
        assert(s.nwords == 1 && s.words[0] == INDEX_op_mb);
        printf("MB %u %u ", mask, s.words[0]);
        for (size_t i = 0; i < s.used; i++) printf("%02x", s.bytes[i]);
        puts("");
    }
}
`;
function uleb(value) {
  const out = [];
  do { const byte = value & 127; value >>>= 7; out.push(byte | (value ? 128 : 0)); } while (value);
  return out;
}
const string = value => [...uleb(Buffer.byteLength(value)), ...Buffer.from(value)];
const section = (id, bytes) => [id, ...uleb(bytes.length), ...bytes];
function moduleBytes(code, shared, memoryAccess = false) {
  const type = memoryAccess ? [1, 0x60, 1, 0x7f, 1, 0x7f] : [1, 0x60, 2, 0x7f, 0x7e, 2, 0x7f, 0x7e];
  // memoryAccess writes a real shared-memory i32, fences, then reads it back.
  // The other function keeps both input values on the stack across the fence.
  const body = memoryAccess
    ? [0, 0x41, 0, 0x20, 0, 0x36, 2, 0, ...code, 0x41, 0, 0x28, 2, 0, 0x0b]
    : [0, 0x20, 0, 0x20, 1, ...code, 0x0b];
  return Uint8Array.from([0, 97, 115, 109, 1, 0, 0, 0,
    ...section(1, type),
    ...(shared ? section(2, [1, ...string('env'), ...string('memory'), 2, 3, 1, 1]) : []),
    ...section(3, [1, 0]), ...section(7, [1, ...string('run'), 0, 0]),
    ...section(10, [1, ...uleb(body.length), ...body])]);
}
const values = [[0, 0n], [-2147483648, -9223372036854775808n], [2147483647, 9223372036854775807n], [-1, -1n]];
const compiler = process.env.BROWSER_QEMU_NATIVE_CC ?? 'cc';
const results = [];
for (const [name, source] of [['baseline', baseline], ['candidate', candidate]]) {
  const actualHelpers = ['tcg_wasm_out8', 'tcg_tci_out32', 'tcg_tci_out_op_v'].map(n => extract(source, n)).join('\n\n');
  const actualGenerator = [extract(sources['tcg-op.c'], 'tcg_gen_mb'), extract(sources['tcg-op-ldst.c'], 'tcg_gen_req_mo')].join('\n\n');
  const emitter = 'static void emit_mb(TCGContext *s, TCGOpcode opc, const TCGArg *args) {\n    (void)args;\n    switch (opc) {\n'+mbCase(source)+'\n    default: assert(0);\n    }\n}';
  const c = [adapter, orderingEnum, hostMO, guestMO, actualHelpers, actualGenerator, emitter, driver].join('\n\n');
  const cPath = join(proof, name+'-emitter.c'), binary = join(temp, name+'-emitter');
  writeFileSync(cPath, c);
  const compile = command(compiler, ['-std=c11', '-O2', '-Wall', '-Wextra', '-Werror', cPath, '-o', binary]);
  writeFileSync(join(proof, name+'-compiler.json'), JSON.stringify(compile, null, 2)+'\n');
  const emitted = command(binary, [], 10_000);
  writeFileSync(join(proof, name+'-emitted.txt'), emitted.stdout);
  const lines = emitted.stdout.trim().split('\n');
  assert.equal(lines.length, 80);
  for (const line of lines.slice(0, 16)) {
    const [, raw, count, type] = line.split(' ').map((v, i) => i ? Number(v) : v);
    const ordering = raw & 0x0d;
    assert.equal(count, ordering ? 1 : 0);
    assert.equal(type, ordering ? 0x30 | ordering : 0);
  }
  assert.ok(lines.includes('REQ 3 1 49'), 'real ordinary-load request emits 0x31');
  assert.ok(lines.includes('REQ 12 1 60'), 'real ordinary-store request emits 0x3c');
  const memory = new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true });
  const memoryBytes = new Uint8Array(memory.buffer);
  memoryBytes.forEach((_, i) => { memoryBytes[i] = i * 17 & 255; });
  const unchanged = Uint8Array.from(memoryBytes);
  let requiredFenceMatches = 0, stackExecutions = 0, sharedMemoryExecutions = 0;
  for (const [index, line] of lines.slice(16).entries()) {
    const [, mask, tag, codeHex] = line.split(' ');
    assert.equal(Number(mask), index); assert.equal(Number(tag), 0xa5);
    if (codeHex === 'fe0300') requiredFenceMatches++;
    assert.equal(codeHex, name === 'candidate' ? 'fe0300' : '01');
    const code = Buffer.from(codeHex, 'hex');
    for (const shared of [false, true]) {
      const wasm = moduleBytes(code, shared);
      assert.ok(WebAssembly.validate(wasm));
      if (index === 0) writeFileSync(join(proof, name+(shared ? '-shared-stack.wasm' : '-no-memory-stack.wasm')), wasm);
      const instance = new WebAssembly.Instance(new WebAssembly.Module(wasm), shared ? { env: { memory } } : {});
      for (const args of values) {
        assert.deepEqual(instance.exports.run(...args), args);
        stackExecutions++;
      }
    }
    assert.deepEqual(memoryBytes, unchanged, 'stack-only fence changes no memory');
    const memoryWasm = moduleBytes(code, true, true);
    assert.ok(WebAssembly.validate(memoryWasm));
    if (index === 0) writeFileSync(join(proof, name+'-shared-write-fence-read.wasm'), memoryWasm);
    const memoryInstance = new WebAssembly.Instance(new WebAssembly.Module(memoryWasm), { env: { memory } });
    for (const [value] of values) {
      assert.equal(memoryInstance.exports.run(value), value);
      assert.equal(new DataView(memory.buffer).getInt32(0, true), value);
      assert.deepEqual(memoryBytes.slice(4), unchanged.slice(4));
      sharedMemoryExecutions++;
    }
    memoryBytes.set(unchanged);
  }
  assert.equal(requiredFenceMatches, name === 'candidate' ? 64 : 0);
  results.push({ name, masks: 64, reqMoCases: 16, requiredFenceMatches,
    stackExecutions, sharedMemoryExecutions, tciWordsPreserved: true });
}
const malformed = [[0xfe, 0x03], [0xfe, 0x03, 0x01], [0xfe, 0x03, 0x30]];
for (const code of malformed) {
  const wasm = moduleBytes(code, false);
  assert.equal(WebAssembly.validate(wasm), false);
  assert.throws(() => new WebAssembly.Module(wasm), WebAssembly.CompileError);
  writeFileSync(join(proof, 'invalid-'+Buffer.from(code).toString('hex')+'.wasm'), wasm);
}
const files = Object.fromEntries(readdirSync(proof).sort().map(name => {
  const bytes = readFileSync(join(proof, name));
  return [name, { bytes: bytes.length, sha256: hash('sha256', bytes) }];
}));
const summary = {
  status: 'passed', kind: 'compiled_c_wasm_memory_barrier',
  qemu_commit: '8604ed49a3cde392890b014a8d5a959c8a2fe72a',
  upstream_blob: blob(originalUpstream), source_blob: blob(baseline), candidate_blob: blob(candidate),
  header_blob: pins['tcg-target.h'], backend_blob: pins['wasm32.c'], source_blobs: pins,
  patch_sha256: hash('sha256', patch),
  checker_sha256: hash('sha256', readFileSync(fileURLToPath(import.meta.url))),
  runtime: { node: process.version, v8: process.versions.v8, platform: process.platform, arch: process.arch },
  patch_apply_reverse: true, results, malformed_fence_controls: malformed.length,
  proof_scope: 'Extracted actual C emitter and req_mo; exact fence encoding; real Wasm validation and stack/shared-memory execution. No inter-thread ordering proof, performance result, full QEMU/VM or browser acceptance.',
  files,
};
writeFileSync(join(proof, 'summary.json'), JSON.stringify(summary, null, 2)+'\n');
console.log(JSON.stringify({ status: summary.status, results, malformedFenceControls: malformed.length,
  proof, summarySha256: hash('sha256', readFileSync(join(proof, 'summary.json'))) }, null, 2));
