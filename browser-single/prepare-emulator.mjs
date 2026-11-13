// Overlay only the verified emulator pair; retain the original guest image.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream, promises as fs} from 'node:fs';
import path from 'node:path';

const [runtime, buildDirectory] = process.argv.slice(2);
assert(runtime && buildDirectory, 'usage: node prepare-emulator.mjs RUNTIME_DIR BUILD_DIR');
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
async function hashFile(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}
const build = JSON.parse(await fs.readFile(path.join(buildDirectory,'build.json'),'utf8'));
for (const [key,value] of Object.entries({
  converter_commit:'087c313479d51cdf255e4f28ded6ae7d8141297b',
  converter_dockerfile_sha256:'f122e849fcb79d718a73dbef6c6cff6b2b71fd3032c8ea7686dc26c007caa508',
  qemu_commit:'8604ed49a3cde392890b014a8d5a959c8a2fe72a',
  backend_postimage_blob:'ccbd0ab09ada98440cda78456cdb2966443ec05d',
  emscripten_version:'4.0.10',
  emsdk_image:'emscripten/emsdk:4.0.10@sha256:90b757eb11fa9a0e3ce4d2d9f76d932a56018e4accc37b5a28b2783751e60eb7',
  max_instance_alive_before:15000,
  max_instance_alive_after:15000,
  max_instance_running_local:2048,
  instance_reservation:'atomic_compare_exchange',
  instance_budget_tests:'passed',
  runtime_diagnostics:'module_lifecycle_counters_v1',
  runtime_diagnostics_tests:'passed',
  i32_multiply_fix:'normalize_operands_before_widening_product',
  multiply_emitter_preimage_blob:'3c85588be97b6c53a451e30c0593e8b433ede7b6',
  multiply_emitter_postimage_blob:'d4078f5adac57d9ccc7d792773fff2ea65a33d6a',
  i32_multiply_tests:'passed',
  instantiate_num:1500
})) assert.equal(build[key],value,`Unexpected emulator ${key}`);
assert.equal(await hashFile(path.join(buildDirectory,'upstream.Dockerfile')),build.converter_dockerfile_sha256,
  'Changed upstream emulator build recipe');
for (const [key,name] of [
  ['patch_sha256','instance-budget.patch'],
  ['builder_sha256','build.sh'],
  ['dockerfile_appendix_sha256','Dockerfile.appendix'],
  ['budget_test_sha256','check-instance-budget.mjs'],
  ['diagnostics_test_sha256','check-runtime-diagnostics.mjs'],
  ['multiply_patch_sha256','i32-multiply.patch'],
  ['multiply_test_sha256','check-i32-multiply.mjs']
]) {
  assert.equal(build[key],await hashFile(`browser-single/qemu/${name}`),`Changed emulator source: ${name}`);
  assert.equal(build[key],await hashFile(path.join(buildDirectory,name)),`Missing emulator source: ${name}`);
}
const proofDirectory = path.join(buildDirectory,'i32-multiply-proof');
const proofBytes = await fs.readFile(path.join(proofDirectory,'summary.json'));
assert.equal(digest(proofBytes),build.multiply_proof_sha256,'Changed multiplication proof');
const proof = JSON.parse(proofBytes);
assert.equal(proof.qemu_commit,build.qemu_commit);
assert.equal(proof.source_blob,build.multiply_emitter_preimage_blob);
assert.equal(proof.candidate_blob,build.multiply_emitter_postimage_blob);
assert.equal(proof.header_blob,'37ad4b93c8ab721639e4cd8603a5dbbe69c07105');
assert.equal(proof.patch_sha256,build.multiply_patch_sha256);
assert.equal(proof.checker_sha256,build.multiply_test_sha256);
assert.deepEqual(proof.baseline,{total:96,passed:54,failed:42});
assert.deepEqual(proof.candidate,{total:96,passed:96,failed:0});
assert.deepEqual((await fs.readdir(proofDirectory)).sort(),[...Object.keys(proof.files),'summary.json'].sort());
for (const [name,expected] of Object.entries(proof.files)) {
  assert(name === path.basename(name) && !['.','..','summary.json'].includes(name),'Unexpected proof path');
  const bytes = await fs.readFile(path.join(proofDirectory,name));
  assert.equal(bytes.length,expected.bytes,`Changed multiplication proof size: ${name}`);
  assert.equal(digest(bytes),expected.sha256,`Changed multiplication proof file: ${name}`);
}
const names = ['out.js','qemu-system-x86_64.wasm'];
assert.deepEqual(Object.keys(build.files ?? {}).sort(),[...names].sort());
const buffers = new Map();
for (const name of names) {
  const bytes = await fs.readFile(path.join(buildDirectory,name));
  assert(bytes.length > 0);
  assert.equal(bytes.length,build.files[name].bytes,`Emulator size mismatch: ${name}`);
  assert.equal(digest(bytes),build.files[name].sha256,`Emulator checksum mismatch: ${name}`);
  buffers.set(name,bytes);
}
assert(WebAssembly.validate(buffers.get('qemu-system-x86_64.wasm')), 'Invalid emulator WebAssembly');
assert(buffers.get('out.js').toString().includes('qemu-system-x86_64.wasm'), 'Unexpected emulator module name');

const preserved = ['qemu-system-x86_64.data','load.js','arg-module.js','kernel-build/bzImage'];
const before = new Map();
for (const name of preserved) before.set(name,await hashFile(path.join(runtime,name)));
for (const [name,bytes] of buffers) await fs.writeFile(path.join(runtime,name),bytes);
for (const [name,expected] of before) assert.equal(await hashFile(path.join(runtime,name)),expected,`Emulator overlay changed ${name}`);

const provenance = path.join(runtime,'emulator-build');
await fs.mkdir(provenance,{recursive:true});
for (const name of ['build.json','SOURCE.txt','instance-budget.patch','build.sh','Dockerfile.appendix',
  'check-instance-budget.mjs','instance-budget-tests.log',
  'check-runtime-diagnostics.mjs','runtime-diagnostics-tests.log',
  'i32-multiply.patch','check-i32-multiply.mjs','i32-multiply-tests.log',
  'upstream.Dockerfile','emscripten-version.txt','config-host.mak','submodules.txt','COPYING','COPYING.LIB','LICENSE']) {
  await fs.copyFile(path.join(buildDirectory,name),path.join(provenance,name));
}
await fs.cp(proofDirectory,path.join(provenance,'i32-multiply-proof'),{recursive:true});
const derivedPath = path.join(runtime,'derived-runtime.json');
const derived = JSON.parse(await fs.readFile(derivedPath,'utf8'));
derived.emulator_build = build;
derived.changes.push('QEMU: evict at 2048 retained instances per CPU; reserve at most 15000 live instances globally');
derived.changes.push('QEMU: passive per-worker module lifecycle counters; original exceptions propagate unchanged');
derived.changes.push('QEMU: normalize i32 operands before signed/unsigned widening multiply; actual emitted Wasm regression checked');
await fs.writeFile(derivedPath,JSON.stringify(derived,null,2)+'\n');
console.log('Verified emulator pair overlaid; guest data, loader, arguments and kernel are unchanged');
