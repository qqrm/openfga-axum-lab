#!/usr/bin/env node
// Actual patched C emits bytes executed by real Wasm; upstream is the negative control.
// No guest image, VM, or full QEMU build.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const [target, proofDirectory, ...extra] = process.argv.slice(2);
assert.ok(target && extra.length === 0,
  'usage: node check-i32-multiply.mjs PATCHED_TCG_TARGET_C [EMPTY_PROOF_DIR]');
const temporaryDirectory = mkdtempSync(join(tmpdir(), 'browser-qemu-multiply-'));
process.on('exit', () => rmSync(temporaryDirectory, { recursive: true, force: true }));
const directory = proofDirectory ? resolve(proofDirectory) : join(temporaryDirectory, 'proof');
mkdirSync(directory, { recursive: true });
assert.equal(readdirSync(directory).length, 0, 'Proof directory must be empty');
const relative = 'tcg/wasm32/tcg-target.c.inc';
const candidate = readFileSync(target, 'utf8');
const header = readFileSync(join(dirname(target), 'tcg-target.h'), 'utf8');
const digest = (kind, data) => createHash(kind).update(data).digest('hex');
const blob = data => digest('sha1', Buffer.concat([Buffer.from('blob ' + Buffer.byteLength(data) + '\0'), Buffer.from(data)]));
assert.equal(blob(candidate), 'd4078f5adac57d9ccc7d792773fff2ea65a33d6a');
assert.equal(blob(header), '37ad4b93c8ab721639e4cd8603a5dbbe69c07105');

function extract(text, name) {
  const pattern = new RegExp('^static(?: inline)? [^\\n]*\\b' + name + '\\([^)]*\\)\\s*\\{', 'gm');
  const matches = [...text.matchAll(pattern)];
  assert.equal(matches.length, 1, 'Exactly one real C function: ' + name);
  const start = matches[0].index;
  let cursor = start + matches[0][0].length, depth = 1;
  // Selected functions contain no braces in strings/comments.
  while (depth && cursor < text.length) {
    const char = text[cursor++];
    if (char === '{') depth++;
    if (char === '}') depth--;
  }
  assert.equal(depth, 0);
  return text.slice(start, cursor);
}
let source = candidate;
for (const [name, extension] of [['mulu2_i32', 'u'], ['muls2_i32', 's']]) {
  const fixed = extract(candidate, 'tcg_wasm_out_' + name);
  let original = fixed;
  for (const operand of ['arg1', 'arg2']) {
    const instruction = '    tcg_wasm_out_op_global_get_r(s, ' + operand + ');';
    const normalized = instruction +
      '\n    tcg_wasm_out_op_i32_wrap_i64(s);' +
      '\n    tcg_wasm_out_op_i64_extend_i32_' + extension + '(s);';
    assert.equal(original.split(normalized).length, 2);
    original = original.replace(normalized, instruction);
  }
  source = source.replace(fixed, original);
}
// Full-file pins prevent a substitute emitter or a mirrored implementation.
assert.equal(blob(source), '3c85588be97b6c53a451e30c0593e8b433ede7b6');
writeFileSync(join(directory, 'upstream-tcg-target.c.inc'), source);
writeFileSync(join(directory, 'patched-tcg-target.c.inc'), candidate);
writeFileSync(join(directory, 'tcg-target.h'), header);
const patch = readFileSync(new URL('./i32-multiply.patch', import.meta.url), 'utf8');
assert.equal(digest('sha256', patch), 'c616ceaba05ed7a01bec6d9af5c0c5dab860a111f95b846c4465162d6d8377a2');
const added = patch.split('\n').filter(line => line.startsWith('+') && !line.startsWith('+++'));
assert.equal(added.length, 8);
assert.ok(added.every(line => /^\+    tcg_wasm_out_op_(i32_wrap_i64|i64_extend_i32_[us])\(s\);$/.test(line)));
assert.equal(patch.split('\n').filter(line => line.startsWith('-') && !line.startsWith('---')).length, 0);
writeFileSync(join(directory, 'i32-multiply.patch'), patch);

const names = [
  'tcg_wasm_out8', 'tcg_wasm_out_leb128_sint64_t',
  'tcg_wasm_out_op_i64_and', 'tcg_wasm_out_op_i64_shr_u', 'tcg_wasm_out_op_i64_mul',
  'tcg_wasm_out_op_i32_wrap_i64', 'tcg_wasm_out_op_var',
  'tcg_wasm_out_op_local_get', 'tcg_wasm_out_op_local_set',
  'tcg_wasm_out_op_global_get', 'tcg_wasm_out_op_global_set',
  'tcg_wasm_out_op_global_get_r', 'tcg_wasm_out_op_global_set_r',
  'tcg_wasm_out_op_i64_const',
  'tcg_wasm_out_op_i64_extend_i32_u', 'tcg_wasm_out_op_i64_extend_i32_s',
  'tcg_wasm_out_op_set_r_as_i64',
  'tcg_wasm_out_mulu2_i32', 'tcg_wasm_out_muls2_i32',
];
const registerEnum = header.match(/typedef enum \{[\s\S]*?\} TCGReg;/)?.[0];
const registerCount = header.match(/^#define TCG_TARGET_NB_REGS \d+$/m)?.[0];
const registerMap = source.match(/static const uint8_t tcg_target_reg_index\[TCG_TARGET_NB_REGS\] = \{[\s\S]*?\n\};/)?.[0];
const temporary = source.match(/^#define TMP64_4_IDX \d+$/m)?.[0];
assert.ok(registerEnum && registerCount && registerMap && temporary);
assert.equal(temporary, '#define TMP64_4_IDX 7');
const adapter = String.raw`
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stddef.h>
/* Only the output sink is adapted; instruction/LEB/register emitters below
 * are extracted verbatim. They access no other TCGContext fields. */
typedef struct { uint8_t bytes[4096]; size_t used; } TCGContext;
static void tcg_sub_out8(TCGContext *s, uint32_t value) {
    assert(s->used < sizeof(s->bytes));
    s->bytes[s->used++] = (uint8_t)value;
}
static bool env_cached;
`;
const driver = String.raw`
int main(void) {
    const struct { const char *name; TCGReg lo, hi, a, b; } layouts[] = {
        {"distinct", 0, 1, 2, 3}, {"aliased", 2, 3, 2, 3},
        {"swapped", 3, 2, 2, 3}, {"square", 0, 1, 2, 2}
    };
    for (size_t mode = 0; mode < 2; mode++) {
        for (size_t i = 0; i < sizeof(layouts) / sizeof(layouts[0]); i++) {
            TCGContext context = {0};
            if (mode == 0)
                tcg_wasm_out_mulu2_i32(&context, layouts[i].lo, layouts[i].hi,
                                     layouts[i].a, layouts[i].b);
            else
                tcg_wasm_out_muls2_i32(&context, layouts[i].lo, layouts[i].hi,
                                     layouts[i].a, layouts[i].b);
            printf("%s %s %d %d %d %d ", mode ? "signed" : "unsigned",
                   layouts[i].name, layouts[i].lo, layouts[i].hi,
                   layouts[i].a, layouts[i].b);
            for (size_t j = 0; j < context.used; j++) printf("%02x", context.bytes[j]);
            puts("");
        }
    }
}
`;

function uleb(value) {
  const bytes = [];
  do { const byte = value & 127; value >>>= 7; bytes.push(byte | (value ? 128 : 0)); } while (value);
  return bytes;
}
const string = value => [...uleb(Buffer.byteLength(value)), ...Buffer.from(value)];
const section = (id, data) => [id, ...uleb(data.length), ...data];
function tinyModule(code) {
  const imports = [...uleb(16)];
  for (let i = 0; i < 16; i++) imports.push(...string('env'), ...string('r' + i), 3, 0x7e, 1);
  // The actual helper uses temporary local index 7, of i64 type.
  const body = [1, 8, 0x7e, ...code, 0x0b];
  return Uint8Array.from([0, 97, 115, 109, 1, 0, 0, 0,
    ...section(1, [1, 0x60, 0, 0]), ...section(2, imports),
    ...section(3, [1, 0]), ...section(7, [1, ...string('run'), 0, 0]),
    ...section(10, [1, ...uleb(body.length), ...body])]);
}
const cases = [
  ['zero', 0n, 0n], ['one', 1n, 1n],
  ['negative_clean', 0xffffffffn, 2n],
  ['minimum_times_two', 0x80000000n, 2n],
  ['maximum_signed', 0x7fffffffn, 0x7fffffffn],
  ['maximum_unsigned', 0xffffffffn, 0xffffffffn],
  ['high_bit_squared', 0x80000000n, 0x80000000n],
  ['mixed', 0x12345678n, 0x87654321n],
  ['polluted_one_two', 0xaaaaaaaa00000001n, 0xbbbbbbbb00000002n],
  ['polluted_negative', 0xfeedfaceffffffffn, 0xdeadbeef00000002n],
  ['polluted_bounds', 0x0123456780000000n, 0xfedcba98ffffffffn],
  ['polluted_zero', 0xffffffff00000000n, 0x12345678ffffffffn],
];
const hex = (value, width = 8) => value.toString(16).padStart(width, '0');
const records = [];
const compiler = process.env.BROWSER_QEMU_NATIVE_CC ?? 'cc';
for (const [variant, text] of [['baseline', source], ['candidate', candidate]]) {
  const actual = names.map(name => extract(text, name)).join('\n\n');
  const native = [adapter, registerCount, registerEnum, registerMap, temporary, actual, driver].join('\n\n');
  const cPath = join(directory, variant + '-emitter.c');
  const binary = join(temporaryDirectory, variant + '-emitter');
  writeFileSync(cPath, native);
  const args = ['-std=c11', '-O0', '-Wall', '-Wextra', '-Werror', '-Wno-unused-function',
    '-Wno-shift-negative-value', cPath, '-o', binary];
  // Pinned signed-LEB code has a negative-shift warning; tested emitters use
  // only positive constants 0xffffffff and 32. Helper bytes stay unmodified.
  const compiled = spawnSync(compiler, args, { encoding: 'utf8', timeout: 30_000 });
  writeFileSync(join(directory, variant + '-compiler.log'), JSON.stringify({compiler,args,status:compiled.status}) + '\n' + compiled.stdout + compiled.stderr);
  assert.ifError(compiled.error);
  assert.equal(compiled.status, 0, compiled.stderr);
  const emitted = spawnSync(binary, [], { encoding: 'utf8', timeout: 10_000 });
  assert.ifError(emitted.error);
  assert.equal(emitted.status, 0, emitted.stderr);
  writeFileSync(join(directory, variant + '-emitted.txt'), emitted.stdout);
  const lines = emitted.stdout.trim().split('\n');
  assert.equal(lines.length, 8);
  for (const line of lines) {
    const [mode, layout, loText, hiText, aText, bText, codeHex] = line.split(' ');
    const [lo, hi, a, b] = [loText, hiText, aText, bText].map(Number);
    assert.ok(['signed', 'unsigned'].includes(mode) && lo !== hi);
    const wasm = tinyModule(Buffer.from(codeHex, 'hex'));
    assert.ok(WebAssembly.validate(wasm), variant + '/' + mode + '/' + layout);
    writeFileSync(join(directory, variant+'-'+mode+'-'+layout+'.wasm'), wasm);
    const globals = Array.from({ length: 16 }, () => new WebAssembly.Global({value:'i64',mutable:true}, 0n));
    const instance = new WebAssembly.Instance(new WebAssembly.Module(wasm),
      {env:Object.fromEntries(globals.map((global,i)=>['r'+i,global]))});
    for (const [name, rawA, originalB] of cases) {
      const rawB = a === b ? rawA : originalB;
      globals.forEach((global,i)=>{global.value=0x5555555500000000n+BigInt(i)});
      globals[a].value=rawA; globals[b].value=rawB;
      const before=globals.map(global=>global.value);
      const interpret = mode === 'signed' ? BigInt.asIntN : BigInt.asUintN;
      const product=BigInt.asUintN(64, interpret(32,rawA)*interpret(32,rawB));
      const expected=[product&0xffffffffn, product>>32n];
      instance.exports.run();
      const result=[BigInt.asUintN(32,globals[lo].value),BigInt.asUintN(32,globals[hi].value)];
      globals.forEach((global,i)=>{if(i!==lo&&i!==hi)assert.equal(global.value,before[i],'Unrelated register changed')});
      const ok=result.every((value,i)=>value===expected[i]);
      const record={variant,mode,layout,case:name,a:hex(rawA,16),b:hex(rawB,16),
        expected:expected.map(value=>hex(value)),actual:result.map(value=>hex(value)),ok};
      records.push(record);
      console.log(JSON.stringify(record));
    }
  }
}
assert.equal(records.filter(r=>r.variant==='baseline').length,96);
assert.ok(records.some(r=>r.variant==='baseline'&&r.mode==='signed'&&r.layout==='distinct'&&r.case==='negative_clean'&&!r.ok));
assert.ok(records.some(r=>r.variant==='baseline'&&r.mode==='unsigned'&&r.case==='polluted_one_two'&&!r.ok));
assert.equal(records.filter(r=>r.variant==='candidate').length,96);
assert.ok(records.filter(r=>r.variant==='candidate').every(r=>r.ok));
const summary = {
  qemu_commit:'8604ed49a3cde392890b014a8d5a959c8a2fe72a',
  source_url:'https://github.com/ktock/qemu-wasm/blob/8604ed49a3cde392890b014a8d5a959c8a2fe72a/'+relative,
  source_blob:blob(source), candidate_blob:blob(candidate), header_blob:blob(header),
  source_sha256:digest('sha256',source), candidate_sha256:digest('sha256',candidate),
  patch_sha256:digest('sha256',patch), checker_sha256:digest('sha256',readFileSync(fileURLToPath(import.meta.url))),
  node_version:process.version, compiler,
  baseline:{total:96,passed:records.filter(r=>r.variant==='baseline'&&r.ok).length,failed:records.filter(r=>r.variant==='baseline'&&!r.ok).length},
  candidate:{total:96,passed:96,failed:0},
  only_source_change:'Eight operand normalization calls in mulu2_i32 and muls2_i32',
  observations:'Actual extracted C emitters and helpers produced all executed instruction bytes; module framing only is a host fixture.',
  limits:'No guest/VM/full QEMU build; not evidence of the cause of browser OOM/timeouts; does not cover other PR54 hunks.',
};
writeFileSync(join(directory,'results.json'),JSON.stringify(records,null,2)+'\n');
summary.files=Object.fromEntries(readdirSync(directory).sort().map(name=>{
  const bytes=readFileSync(join(directory,name));
  return [name,{bytes:bytes.length,sha256:digest('sha256',bytes)}];
}));
writeFileSync(join(directory,'summary.json'),JSON.stringify(summary,null,2)+'\n');
console.log('PASS baseline reproduces signed clean-upper defect and unsigned polluted-upper defect');
console.log('PASS candidate passes 96 actual-Wasm cases including bounds, polluted upper bits, and aliases');
console.log(JSON.stringify(summary,null,2));
