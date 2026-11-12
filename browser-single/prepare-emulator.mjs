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
  emscripten_version:'4.0.10',
  emsdk_image:'emscripten/emsdk:4.0.10@sha256:90b757eb11fa9a0e3ce4d2d9f76d932a56018e4accc37b5a28b2783751e60eb7',
  max_instance_alive_before:15000,
  max_instance_alive_after:4096,
  instantiate_num:1500
})) assert.equal(build[key],value,`Unexpected emulator ${key}`);
assert.equal(await hashFile(path.join(buildDirectory,'upstream.Dockerfile')),build.converter_dockerfile_sha256,
  'Changed upstream emulator build recipe');
for (const [key,name] of [
  ['patch_sha256','instance-budget.patch'],
  ['builder_sha256','build.sh'],
  ['dockerfile_appendix_sha256','Dockerfile.appendix']
]) {
  assert.equal(build[key],await hashFile(`browser-single/qemu/${name}`),`Changed emulator source: ${name}`);
  assert.equal(build[key],await hashFile(path.join(buildDirectory,name)),`Missing emulator source: ${name}`);
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
  'upstream.Dockerfile','emscripten-version.txt','config-host.mak','submodules.txt','COPYING','COPYING.LIB','LICENSE']) {
  await fs.copyFile(path.join(buildDirectory,name),path.join(provenance,name));
}
const derivedPath = path.join(runtime,'derived-runtime.json');
const derived = JSON.parse(await fs.readFile(derivedPath,'utf8'));
derived.emulator_build = build;
derived.changes.push('QEMU compiled WebAssembly instance budget: 15000 to 4096');
await fs.writeFile(derivedPath,JSON.stringify(derived,null,2)+'\n');
console.log('Verified emulator pair overlaid; guest data, loader, arguments and kernel are unchanged');
