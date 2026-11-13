// Diagnostic materialization only: no builds and no execution of artifact scripts.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream, promises as fs} from 'node:fs';
import {spawnSync} from 'node:child_process';
import path from 'node:path';

const output = path.resolve('artifacts/firefox-kernel-comparison');
await fs.mkdir(output,{recursive:true});
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
async function fileRecord(file) {
  const hash = createHash('sha256');
  let bytes = 0;
  for await (const chunk of createReadStream(file)) {bytes += chunk.length; hash.update(chunk);}
  return {bytes,sha256:hash.digest('hex')};
}
const json = async file => JSON.parse(await fs.readFile(file,'utf8'));
const common = {
  source:'99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58',
  data:{bytes:550511872,sha256:'5735f9936e760f467571a4ee053ef56c0e3404709586a85e37d6d25c57248ef4'},
  loader:'ea9d077f640bf9e2a368193b161f48a9d268e4c4eae4ac9d8a78d318474c73d9',
  kernel:'ac8d4da19cd4bcafe99bb9122ac1b6598920661fac3ad72f5fe53b62400bbd19',
  args:{bytes:739,sha256:'848ee06345b05f6a335536b6c514c510e6eddca1a0ba5121b36f8613d6de5ec8'},
  glue:{bytes:279363,sha256:'95f7e6b59d98cd7267e705aa0b2cd670970b984d13c7e81b4d4f677cd65534d2'}
};
const specs = [
  {label:'old',head:'6934232bb04036b75aef6383008ceb6db14ad928',
    wasm:{bytes:41246842,sha256:'01fd2f34c1e5774b6f12ce328c5ea13181b57338cdf91ea4a85340521a5058c2'}},
  {label:'new',head:'5d18ed2fda49fe84a9ac03c7b8571940f0891819',
    wasm:{bytes:41246941,sha256:'e85450e88a390c3494c9ce4d8fb599c8ed14afd8ae40b48deeee1bc6b3f34c0e'}}
];
assert.equal((await fileRecord('prepared/kernel/bzImage')).sha256,common.kernel);
const kernelConfig = await fs.readFile('prepared/kernel/linux.config','utf8');
for (const flag of ['CONFIG_SYSVIPC=y','CONFIG_IPC_NS=y','CONFIG_HZ_100=y','CONFIG_HZ_PERIODIC=y']) {
  assert(kernelConfig.split('\n').includes(flag),'Missing retained kernel setting: '+flag);
}
for (const name of ['e2e/package.json','e2e/package-lock.json','fga/model.json',
                    'browser-single/pages/log-stream.mjs','browser-single/repack-runtime.mjs',
                    'browser-single/pages/guest.sh','browser-single/entrypoint.sh']) {
  assert.equal((await fileRecord(path.join('source-old',name))).sha256,
               (await fileRecord(path.join('source-new',name))).sha256,
               'Unexpected common source difference: '+name);
}
const prepared = [];
for (const spec of specs) {
  const source = path.resolve('source-'+spec.label);
  const directory = path.resolve('prepared',spec.label);
  const wasm = path.join(directory,'browser-output/wasm');
  const runtime = path.join(wasm,'runtime');
  const recordDir = path.join(output,spec.label+'-provenance');
  await fs.mkdir(recordDir,{recursive:true});
  const head = spawnSync('git',['-C',source,'rev-parse','HEAD'],{encoding:'utf8',timeout:10000});
  assert.equal(head.status,0,'Cannot verify source checkout');
  assert.equal(head.stdout.trim(),spec.head,'Source checkout changed');
  const archived = await json(path.join(directory,'archive-provenance.json'));
  assert.equal(archived.head,spec.head);
  const retained = await json(path.join(directory,'retained/derived-runtime.json'));
  const startupBytes = await fs.readFile(path.join(directory,'retained/startup-manifest.json'));
  const startup = JSON.parse(startupBytes);
  const emulator = await json(path.join(directory,'emulator/build.json'));
  const openfga = await json(path.join(directory,'openfga/build.json'));
  assert.equal(retained.source,common.source);
  assert.equal(retained.rootfs_changed,false);
  assert.deepEqual(retained.data,common.data);
  assert.equal(retained.loader_sha256,common.loader);
  assert.equal(retained.kernel_sha256,common.kernel);
  assert.deepEqual(retained.emulator_build,emulator);
  assert.deepEqual(startup.emulator_build,emulator);
  assert.deepEqual(startup.openfga_build,openfga);
  assert.equal(startup.base_source_commit,common.source);
  assert.equal(retained.startup_overlay.manifest,'startup-manifest.json');
  assert.equal(retained.startup_overlay.sha256,sha(startupBytes));
  assert.equal(emulator.qemu_commit,'8604ed49a3cde392890b014a8d5a959c8a2fe72a');
  assert.equal(emulator.backend_postimage_blob,'e0b26bdc8d6d5b987061f9090ae0b62b76c34e16');
  assert.equal(emulator.instance_budget_configuration,'shared_c_pre_run_setter');
  assert.equal(emulator.firefox_instance_limit,8192);
  assert.equal(emulator.chromium_instance_limit,15000);
  assert.equal(emulator.max_instance_running_local,2048);
  assert.equal(emulator.instantiate_num,1500);
  assert.equal(openfga.warmup_model_sha256,(await fileRecord(path.join(source,'fga/model.json'))).sha256);
  assert.deepEqual(emulator.files,{'out.js':common.glue,'qemu-system-x86_64.wasm':spec.wasm});
  for (const [name,expected] of Object.entries(emulator.files)) {
    assert.deepEqual(await fileRecord(path.join(directory,'emulator',name)),expected,'Wrong emulator pair member: '+name);
  }
  // Both emulator variants use exactly the same current host/launcher assets.
  // The guest scripts must also match both source refs and retained payload hashes.
  await fs.cp(path.resolve('source-new/browser-single/pages'),wasm,{recursive:true});
  await fs.copyFile(path.resolve('source-new/browser-single/entrypoint.sh'),path.join(wasm,'entrypoint.sh'));
  await fs.copyFile(path.join(directory,'openfga/openfga'),path.join(runtime,'openfga'));
  assert.deepEqual(Object.keys(startup.files).sort(),['entrypoint.sh','guest.sh','runtime/openfga']);
  for (const [name,expected] of Object.entries(startup.files)) {
    assert.deepEqual(await fileRecord(path.join(wasm,name)),expected,'Startup overlay changed: '+name);
  }
  assert.equal(openfga.binary_sha256,startup.files['runtime/openfga'].sha256);
  // Execute only this pinned repository script, never a script from an artifact.
  const repack = spawnSync(process.execPath,[path.join(source,'browser-single/repack-runtime.mjs'),
    runtime,path.resolve('prepared/kernel')],{encoding:'utf8',timeout:60000,maxBuffer:1024*1024});
  await fs.writeFile(path.join(recordDir,'repack.log'),(repack.stdout??'')+(repack.stderr??''));
  assert.equal(repack.status,0,'Deterministic kernel repack failed');
  const rebuilt = await json(path.join(runtime,'derived-runtime.json'));
  for (const key of ['source','linux_source','rootfs_changed','data','loader_sha256','kernel_sha256']) {
    assert.deepEqual(rebuilt[key],retained[key],'Repacked identity differs: '+key);
  }
  assert.deepEqual(await fileRecord(path.join(runtime,'qemu-system-x86_64.data')),common.data);
  assert.equal((await fileRecord(path.join(runtime,'load.js'))).sha256,common.loader);
  assert.deepEqual(await fileRecord(path.join(runtime,'arg-module.js')),common.args);
  assert.equal((await fileRecord(path.join(runtime,'kernel-build/bzImage'))).sha256,common.kernel);
  for (const name of Object.keys(emulator.files)) {
    await fs.copyFile(path.join(directory,'emulator',name),path.join(runtime,name));
    assert.deepEqual(await fileRecord(path.join(runtime,name)),emulator.files[name]);
  }
  await fs.writeFile(path.join(runtime,'startup-manifest.json'),startupBytes);
  await fs.copyFile(path.join(directory,'retained/derived-runtime.json'),path.join(runtime,'derived-runtime.json'));
  const files = {};
  for (const name of ['launcher.js','guest.sh','entrypoint.sh','isolation-sw.js','log-stream.mjs','qemu-budget.mjs',
                      'runtime/startup-manifest.json','runtime/derived-runtime.json','runtime/openfga',
                      'runtime/out.js','runtime/qemu-system-x86_64.wasm','runtime/load.js','runtime/arg-module.js']) {
    files[name] = await fileRecord(path.join(wasm,name));
  }
  files['runtime/qemu-system-x86_64.data'] = common.data;
  for (const name of ['archive-provenance.json','retained/startup-manifest.json','retained/derived-runtime.json',
                      'emulator/build.json','openfga/build.json']) {
    await fs.copyFile(path.join(directory,name),path.join(recordDir,name.replaceAll('/','-')));
  }
  prepared.push({...spec,root:path.dirname(wasm),files,archived});
}
await fs.copyFile('prepared/kernel/archive-provenance.json',path.join(output,'kernel-archive-provenance.json'));
const result = {diagnosticOnly:true,productionAcceptance:false,scope:'exact retained bundle kernel boot comparison',
  bundleMode:'common_5d18_host_exact_retained_runtime',hostSourceHead:'5d18ed2fda49fe84a9ac03c7b8571940f0891819',
  buildsPerformed:false,common,prepared,sourceLogModule:await fileRecord('source-new/browser-single/pages/log-stream.mjs'),
  runner:await fileRecord('diagnostics/check-firefox-kernel-boot.mjs'),
  materializer:await fileRecord('diagnostics/prepare-firefox-kernel-comparison.mjs')};
await fs.writeFile(path.join(output,'prepared.json'),JSON.stringify(result,null,2)+'\n');
console.log('DIAGNOSTIC_ONLY: two retained bundles reconstructed and byte identities verified; no VM started.');
