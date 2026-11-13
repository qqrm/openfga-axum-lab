// Diagnostic materialization only. No emulator/service build and no VM startup.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream, promises as fs} from 'node:fs';
import path from 'node:path';
import {spawnSync} from 'node:child_process';

const sourceCommit = 'd7f7ead003c774e30584798adfde38afb854528d';
const source = path.resolve('source-candidate');
const prepared = path.resolve('prepared');
const evidence = path.resolve('artifacts/bootstrap-browser-gate');
const runtime = path.join(source,'browser-output/wasm/runtime');
const readJSON = async filename => JSON.parse(await fs.readFile(filename,'utf8'));
const fileRecord = async filename => {
  const hash = createHash('sha256');
  let bytes = 0;
  for await (const block of createReadStream(filename)) { bytes += block.length; hash.update(block); }
  return {bytes,sha256:hash.digest('hex')};
};
function command(program,args,options={}) {
  const result = spawnSync(program,args,{cwd:source,encoding:'utf8',stdio:'inherit',...options});
  assert.equal(result.error,undefined,program+' failed to start');
  assert.equal(result.signal,null,program+' terminated by signal');
  assert.equal(result.status,0,program+' failed');
  return result.stdout;
}
async function metadata(directory) {
  const loader = await fs.readFile(path.join(directory,'load.js'),'utf8');
  const records = [...loader.matchAll(/loadPackage\((\{"files":.*\})\);/g)];
  assert.equal(records.length,1,'Expected one runtime segment table');
  const result = JSON.parse(records[0][1]);
  assert.equal(result.remote_package_size,(await fs.stat(path.join(directory,'qemu-system-x86_64.data'))).size);
  let cursor = 0;
  for (const file of result.files) {
    assert.equal(file.start,cursor);
    assert(Number.isSafeInteger(file.end) && file.end > file.start);
    cursor = file.end;
  }
  assert.equal(cursor,result.remote_package_size);
  assert.equal(new Set(result.files.map(file=>file.filename)).size,result.files.length);
  return result;
}
async function segmentRecord(directory,segment) {
  const hash = createHash('sha256');
  let bytes = 0;
  for await (const block of createReadStream(path.join(directory,'qemu-system-x86_64.data'),
    {start:segment.start,end:segment.end-1})) {
    bytes += block.length;
    hash.update(block);
  }
  assert.equal(bytes,segment.end-segment.start);
  return {bytes,sha256:hash.digest('hex')};
}
await fs.mkdir(evidence,{recursive:true});
assert.equal(command('git',['rev-parse','HEAD'],{stdio:'pipe'}).trim(),sourceCommit);
command('git',['diff','--exit-code']);
const originalManifest = await readJSON(path.join(source,'browser-single/pages/runtime-manifest.json'));
assert.equal(originalManifest.source_commit,'99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58');
assert.equal(originalManifest.archive_sha256,'fd7511caa98399d245adce39feb3398884f8dc56a82aa44c625e6fa46bf0acee');
const retained = await readJSON(path.join(prepared,'retained/derived-runtime.json'));
assert.deepEqual(retained.data,{bytes:550511872,sha256:'5735f9936e760f467571a4ee053ef56c0e3404709586a85e37d6d25c57248ef4'});
assert.equal(retained.loader_sha256,'ea9d077f640bf9e2a368193b161f48a9d268e4c4eae4ac9d8a78d318474c73d9');
assert.equal(retained.kernel_sha256,'ac8d4da19cd4bcafe99bb9122ac1b6598920661fac3ad72f5fe53b62400bbd19');
assert.equal(retained.rootfs_changed,false);
const previousStartup = await readJSON(path.join(prepared,'retained/startup-manifest.json'));
const openfga = await readJSON(path.join(source,'artifacts/browser-openfga/build.json'));
const emulator = await readJSON(path.join(source,'artifacts/browser-emulator/build.json'));
assert.deepEqual(previousStartup.openfga_build,openfga);
assert.deepEqual(previousStartup.emulator_build,emulator);
assert.equal(previousStartup.base_source_commit,originalManifest.source_commit);

const pair = {
  'out.js':{bytes:279363,sha256:'95f7e6b59d98cd7267e705aa0b2cd670970b984d13c7e81b4d4f677cd65534d2'},
  'qemu-system-x86_64.wasm':{bytes:41246941,sha256:'e85450e88a390c3494c9ce4d8fb599c8ed14afd8ae40b48deeee1bc6b3f34c0e'}
};
for (const [name,expected] of Object.entries(pair)) {
  assert.deepEqual(await fileRecord(path.join(source,'artifacts/browser-emulator',name)),expected);
}
const kernelFiles = {
  'bzImage':{bytes:3628928,sha256:'4d2088c41c19fa04d6ac93f97105d8541831acfcc2e96feb07e581fcd2a3db48'},
  'linux.config':{bytes:75338,sha256:'fbee1ad0bc5fb73f2ca82ba98fa045b76ff19e67a148b40cb93b63916c2ad9b8'},
  'ipc-baseline.config':{bytes:72396,sha256:'b109d75227af31a6128280fd192bc06aa1e3d290075a2cf1c2e4fe148df00fcc'},
  'acpi.config.diff':{bytes:1707,sha256:'84520b160eab472b1db6d6b028bd3f0bae19dfda5498d0d914699d514dfd388c'},
  'SOURCE.txt':{bytes:313,sha256:'c21dda73d592ffe967c6a6b663ac6d02b116564cb93cad96f0e17c94266778eb'},
  'build.sha256':{bytes:342,sha256:'a2886a81e725f7599d964b94d6aabb26ac1fb1a289fc3f6ceed8d518f4cdc525'}
};
for (const [name,expected] of Object.entries(kernelFiles)) {
  assert.deepEqual(await fileRecord(path.join(source,'artifacts/browser-kernel',name)),expected);
}
const originalDirectory = path.join(prepared,'original-runtime');
for (const [name,expected] of Object.entries(originalManifest.files)) {
  assert.deepEqual(await fileRecord(path.join(originalDirectory,name)),expected);
}
const originalMetadata = await metadata(originalDirectory);
const originalSegments = {};
for (const segment of originalMetadata.files) {
  if (!['/pack/bzImage','/pack/vm.state'].includes(segment.filename)) {
    originalSegments[segment.filename] = await segmentRecord(originalDirectory,segment);
  }
}
const rootfsEntries = Object.entries(originalSegments).filter(([,record])=>record.bytes===546670592);
assert.equal(rootfsEntries.length,1,'Expected the known application filesystem segment');
assert.equal(rootfsEntries[0][1].sha256,'beb2efc092ed90e5e9919ff4f1f33f7d239aeca19a322025467f7406b09302b5');
assert.deepEqual(await fileRecord(path.join(source,'fga/model.json')),
  {bytes:52709,sha256:'a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d'});

// The production materializer requires its model-explorer HTML input. Copy
// the reviewed source HTML; this experiment does not build or test that demo.
await fs.mkdir(path.join(source,'browser-output'),{recursive:true});
await fs.copyFile(path.join(source,'browser-demo/index.html'),path.join(source,'browser-output/index.html'));
command('bash',['browser-single/prepare-pages.sh'],{
  env:{...process.env,BROWSER_RUNTIME_ARCHIVE:path.join(prepared,'runtime-source.zip')}
});
command('git',['diff','--exit-code']);

const derived = await readJSON(path.join(runtime,'derived-runtime.json'));
const expectedData = {bytes:550772608,sha256:'0260827c6a39e526dd180f76b754220892ac7d0799ed8503d2ab564795bb90a5'};
const expectedLoader = {bytes:7521,sha256:'24388c26e7fd354d937b0c8bc66f2f0a8e2ac0bcfd1b17e7f8771862399ce3b0'};
assert.deepEqual(await fileRecord(path.join(runtime,'qemu-system-x86_64.data')),expectedData);
assert.deepEqual(await fileRecord(path.join(runtime,'load.js')),expectedLoader);
assert.deepEqual(derived.data,expectedData);
assert.equal(derived.loader_sha256,expectedLoader.sha256);
assert.equal(derived.kernel_sha256,kernelFiles.bzImage.sha256);
assert.equal(derived.rootfs_changed,false);
assert.deepEqual(await fileRecord(path.join(runtime,'arg-module.js')),originalManifest.files['arg-module.js']);
for (const [name,expected] of Object.entries(pair)) {
  assert.deepEqual(await fileRecord(path.join(runtime,name)),expected);
}
const derivedMetadata = await metadata(runtime);
assert.equal(derivedMetadata.files.some(file=>file.filename==='/pack/vm.state'),false);
assert.equal(derivedMetadata.files.length,originalMetadata.files.length-1);
const preservedSegments = {};
for (const segment of derivedMetadata.files) {
  const actual = await segmentRecord(runtime,segment);
  if (segment.filename==='/pack/bzImage') assert.deepEqual(actual,kernelFiles.bzImage);
  else {
    assert.deepEqual(actual,originalSegments[segment.filename],'Application segment changed: '+segment.filename);
    preservedSegments[segment.filename] = actual;
  }
}
const startup = await readJSON(path.join(runtime,'startup-manifest.json'));
assert.deepEqual(startup.openfga_build,openfga);
assert.deepEqual(startup.emulator_build,emulator);
assert.deepEqual(Object.keys(startup.files).sort(),
  ['guest.sh','entrypoint.sh','runtime/openfga','runtime/bootstrap'].sort());
assert.equal(startup.bootstrap_build.source_commit,'53ff85cfdd3d84c26ae432189790e1e0f72d61db');
assert.equal(startup.bootstrap_build.native_tests,'passed');
assert.equal(startup.bootstrap_build.browser_model_timeout_seconds,30);
assert.equal(derived.startup_overlay.sha256,(await fileRecord(path.join(runtime,'startup-manifest.json'))).sha256);
const sourceFiles = {};
for (const name of ['browser-single/prepare-pages.sh','browser-single/check-pages.mjs',
  'browser-single/pages/launcher.js','browser-single/pages/startup-payload.mjs',
  'browser-single/pages/guest.sh','browser-single/entrypoint.sh',
  'e2e/package-lock.json','fga/model.json']) {
  sourceFiles[name] = await fileRecord(path.join(source,name));
}
const archives = {};
for (const label of ['retained','kernel','bootstrap']) {
  archives[label] = await readJSON(path.join(prepared,label+'-archive-provenance.json'));
}
const proof = {
  schemaVersion:1,experiment:'bootstrap_model_budget_30',diagnosticOnly:true,
  modelExplorerBuildVerified:false,sourceCommit,sourceFiles,archives,
  retainedRuntimeSource:'379e3280c2a1c789071292af141648eb2fc3ffbc',
  originalRuntimeSource:originalManifest.source_commit,
  kernelSource:'af9c5555e312ffaec3785154f5be83be7469c95f',
  rootfsUnchanged:true,preservedSegments,data:expectedData,loader:expectedLoader,
  args:originalManifest.files['arg-module.js'],kernel:kernelFiles.bzImage,emulator:pair,
  startupManifest:await fileRecord(path.join(runtime,'startup-manifest.json')),
  bootstrapBuild:startup.bootstrap_build,
  executions:{initialBootstrap:'/tmp/browser-lab-runtime/bootstrap',
    probe:'/usr/local/bin/lab-tool',acceptance:'/usr/local/bin/lab-tool'},
  expectedRuntimeConfiguration:{virtualCPUs:2,memoryMiB:512,accel:'tcg,tb-size=500,thread=multi'},
  authorizationDeadlinesChanged:false
};
await fs.writeFile(path.join(evidence,'materialization.json'),JSON.stringify(proof,null,2)+'\n');
console.log('BOOTSTRAP_BROWSER_GATE '+JSON.stringify({phase:'materialized',sourceCommit,
  rootfsUnchanged:true,data:expectedData,loader:expectedLoader,kernel:kernelFiles.bzImage,
  bootstrapSource:startup.bootstrap_build.source_commit,bootstrapSha256:startup.bootstrap_build.binary_sha256,
  modelExplorerBuildVerified:false}));
