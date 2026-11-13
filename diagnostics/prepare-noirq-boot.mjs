// Diagnostic materialization only. No emulator/service build and no VM startup.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream, promises as fs} from 'node:fs';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import {runInNewContext} from 'node:vm';

const sourceCommit = '1f10fb8b17cf09ae18b0003fbf0b7e731b91b6aa';
const nativeProducerCommit = 'b267fefdba375395d496e6d89413786c048c70d2';
const baselineProductionCommit = '55fb279741aa47ae813e5b7d5aff73dbd2692cc1';
const source = path.resolve('source-candidate');
const prepared = path.resolve('prepared');
const evidence = path.resolve('artifacts/noirq-boot-diagnostic');
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
// All six runtime payload/production-checker files remain exact d7 bytes.
const fixtureFiles = {
  "browser-single/entrypoint.sh": {
    "gitBlob": "828a107cb517edc1d938a83c996d4017c5506b99",
    "bytes": 5367,
    "mode": "100755"
  },
  "browser-single/pages/guest.sh": {
    "gitBlob": "ff07c8f9ce78da7e366321c8b755033d21b61ff9",
    "bytes": 6155,
    "mode": "100755"
  },
  "browser-single/pages/startup-payload.mjs": {
    "gitBlob": "6078c754c273a82e5b1d5d29156b1aa472e5ad94",
    "bytes": 3287,
    "mode": "100644"
  },
  "browser-single/startup-payload.test.mjs": {
    "gitBlob": "4fef07ea20a3306f9c03ea6d5dbcbd95d75161f1",
    "bytes": 7001,
    "mode": "100644"
  },
  "browser-single/prepare-pages.sh": {
    "gitBlob": "ad7b86f56707d78371b89d4435e35af61fa234d6",
    "bytes": 8469,
    "mode": "100755"
  },
  "browser-single/check-pages.mjs": {
    "gitBlob": "6a0de543019051b44b708f837f6cf8d656fa9d29",
    "bytes": 11835,
    "mode": "100644"
  }
};
const launcherPath = 'browser-single/pages/launcher.js';
assert.equal(command('git',['rev-parse','HEAD^'],{stdio:'pipe'}).trim(),baselineProductionCommit);
const changedPaths = command('git',['diff','--name-only',baselineProductionCommit,'HEAD'],{stdio:'pipe'}).trim().split('\n');
assert.deepEqual(changedPaths,[launcherPath],'The candidate must change only the reviewed kernel argument mapping');
const baselineLauncher = command('git',['show',baselineProductionCommit+':'+launcherPath],{stdio:'pipe',encoding:null});
assert(Buffer.isBuffer(baselineLauncher));
const launcher = await fs.readFile(path.join(source,launcherPath));
const replacements = [
  ["kernelFlags.filter(flag => flag !== 'slub_debug=F' && flag !== 'acpi=off')",
   "kernelFlags.filter(flag => flag !== 'slub_debug=F')"],
  [".map(flag => flag === 'loglevel=0' ? 'loglevel=7' : flag).join(' ');",
   ".map(flag => flag === 'loglevel=0' ? 'loglevel=7' : flag === 'acpi=off' ? 'acpi=noirq' : flag).join(' ');"]
];
let inverse = launcher.toString('utf8');
for (const [before,after] of replacements) {
  assert.equal(inverse.split(after).length,2,'Expected one exact reviewed launcher replacement');
  inverse = inverse.replace(after,()=>before);
}
assert.deepEqual(Buffer.from(inverse),baselineLauncher,'Reversing only two reviewed lines must restore the exact parent');
assert.equal(command('git',['hash-object','--',launcherPath],{stdio:'pipe'}).trim(),'ea8e9a477a48b41d4137ad38b322977fd65dbda0');
for (const [name,expected] of Object.entries(fixtureFiles)) {
  assert.equal(command('git',['hash-object','--',name],{stdio:'pipe'}).trim(),expected.gitBlob,
    'Reviewed production runtime file changed: '+name);
  assert.equal((await fs.stat(path.join(source,name))).size,expected.bytes);
  assert.equal(command('git',['ls-tree','HEAD','--',name],{stdio:'pipe'}).split(' ')[0],expected.mode);
}
const configurationExperiment = {
  variable:'kernel_acpi_irq_routing',value:'acpi=noirq',engine:'firefox',
  runtimeSource:sourceCommit,baselineProductionCommit,nativeProducerCommit,
  nativeConfigurationProof:false,inverseByteComparison:true,changedPaths,
  baselineLauncher:{bytes:baselineLauncher.length,sha256:createHash('sha256').update(baselineLauncher).digest('hex')},
  candidateLauncher:{bytes:launcher.length,sha256:createHash('sha256').update(launcher).digest('hex')},
  guest:await fileRecord(path.join(source,'browser-single/pages/guest.sh')),
  unchangedRequestDeadlines:true,unchangedModelAndBinaries:true,
  scope:'One kernel routing hypothesis; boot observation only, no backend acceptance claim'
};
const archives = {};
for (const label of ['retained','kernel','native','bootstrap']) {
  archives[label] = await readJSON(path.join(prepared,label+'-archive-provenance.json'));
}
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
// The old .7 OpenFGA is provenance only. This run must use the new native-tested .8.
const nativeArchive = archives.native;
const bootstrapArchive = archives.bootstrap;
assert(Number.isSafeInteger(nativeArchive.run) && nativeArchive.run>0);
assert.match(nativeArchive.head,/^[0-9a-f]{40}$/);
assert.equal(nativeArchive.run,bootstrapArchive.run);
assert.equal(nativeArchive.head,bootstrapArchive.head);
assert.notEqual(nativeArchive.artifact,bootstrapArchive.artifact);
assert.equal(nativeArchive.artifact_name,'browser-openfga8-native-evidence');
assert.equal(bootstrapArchive.artifact_name,'browser-bootstrap-openfga8');
const nativeRoot = path.join(prepared,'native-evidence');
const inputs = await readJSON(path.join(nativeRoot,'openfga8-verifier-input.json'));
for (const [key,value] of Object.entries({
  producerSource:nativeProducerCommit,rustSource:'53ff85cfdd3d84c26ae432189790e1e0f72d61db',
  packagingSource:'d7f7ead003c774e30584798adfde38afb854528d',
  workflowCommit:nativeArchive.head,runId:String(nativeArchive.run),runAttempt:'1',
  repositoryRuntimeContract:'passed',scriptSyntaxAndShellCheck:'passed',browserAcceptanceVerified:false
})) assert.equal(inputs[key],value,'Native verifier input differs: '+key);
const inputFiles = ['browser-single/OpenFGA.Dockerfile','browser-single/openfga/build.sh',
  'browser-single/bootstrap/build.sh','browser-single/bootstrap/native-proof.mjs'];
assert.deepEqual(Object.keys(inputs.sourceSha256).sort(),inputFiles.slice().sort());
for (const name of inputFiles) {
  assert.equal(inputs.sourceSha256[name],(await fileRecord(path.join(source,name))).sha256);
}
const openfgaDirectory = path.join(source,'artifacts/browser-openfga');
const openfgaBinary = await fileRecord(path.join(openfgaDirectory,'openfga'));
assert(openfgaBinary.bytes>0 && openfgaBinary.bytes<=128*1024*1024);
assert.equal(openfga.binary_sha256,openfgaBinary.sha256);
for (const [key,value] of Object.entries({
  source_commit:'293893b22a53d8a130a5f7c97cab4bc8e91bcaa4',upstream_version:'1.10.3',
  target:'linux/amd64',model_json_encoder:'single_exact_real_model_response',
  first_resolver_typesystem_reuse:'one_shot_after_real_model_read',
  model_reuse_race_tests:'passed',model_codec_tests:'passed',model_validation_tests:'passed',
  startup_dial_timeout_seconds:60,warmup_timeout_seconds:60,warmup_cycles:1,
  authorization_request_timeout_default_seconds:3
})) assert.equal(openfga[key],value,'New OpenFGA build contract differs: '+key);
const sourceBindings = {
  patch_sha256:'startup-dial.patch',helper_sha256:'browser_http_dial.go',
  tests_sha256:'browser_http_dial_test.go',warmup_helper_sha256:'browser_http_warmup.go',
  warmup_tests_sha256:'browser_http_warmup_test.go',model_codec_helper_sha256:'browser_model_codec.go',
  model_codec_tests_sha256:'browser_model_codec_test.go',
  model_codec_integration_tests_sha256:'browser_model_codec_integration_test.go',
  model_validation_helper_sha256:'browser_model_validation.go',
  model_validation_tests_sha256:'browser_model_validation_test.go',
  write_typesystem_tests_sha256:'browser_write_typesystem_test.go',
  resolver_transfer_integration_tests_sha256:'browser_resolver_transfer_integration_test.go'
};
const exportedSources = new Set(['startup-dial.patch','browser_http_dial.go','browser_http_warmup.go',
  'browser_model_validation.go','browser_model_validation_test.go','browser_model_codec.go',
  'browser_model_codec_test.go','browser_model_codec_integration_test.go',
  'browser_write_typesystem_test.go','browser_resolver_transfer_integration_test.go']);
for (const [key,name] of Object.entries(sourceBindings)) {
  const actual = await fileRecord(path.join(source,'browser-single/openfga',name));
  assert.equal(openfga[key],actual.sha256,'OpenFGA source/test hash differs: '+name);
  if (exportedSources.has(name)) {
    assert.deepEqual(await fileRecord(path.join(openfgaDirectory,name)),actual,'Exported proof source differs: '+name);
  }
}
const raceLog = await fileRecord(path.join(openfgaDirectory,'model-reuse-race-tests.log'));
assert(raceLog.bytes>0);
assert.equal(openfga.model_reuse_race_tests_sha256,raceLog.sha256,'Race log hash differs');
// The successful native retry reuses the exact Go-tested output from step 7.
const originPath = path.join(nativeRoot,'openfga8-retained-origin.json');
const originRecord = await fileRecord(originPath);
assert.equal(inputs.retainedOpenfgaOriginSha256,originRecord.sha256,'Retained origin is not bound to current native input');
const origin = await readJSON(originPath);
for (const [key,value] of Object.entries({
  schema_version:1,source_run:43440821229,source_job:131937646033,
  source_workflow_commit:'84112f7373df40dc767d7f10fbea453720651f0e',
  source_commit:'501cfc2535f09826550aae97225fa5cbda86f08d',
  source_run_attempt:1,source_run_conclusion:'failure',
  artifact:14047585163,artifact_name:'browser-openfga8-native-evidence',
  browser_acceptance_verified:false
})) assert.equal(origin[key],value,'Retained Go origin differs: '+key);
assert.deepEqual(origin.archive,{bytes:20612111,sha256:'45fda236fb2dc536d0c437e38230a18f8a19fc09b8b7fca6807cc76f65fc094f'});
assert.equal(origin.openfga_build_step.number,7);
assert.equal(origin.openfga_build_step.name,'Build and regression-test the actual combined OpenFGA producer');
assert.equal(origin.openfga_build_step.conclusion,'success');
const goStarted = Date.parse(origin.openfga_build_step.started_at);
const goCompleted = Date.parse(origin.openfga_build_step.completed_at);
assert(Number.isFinite(goStarted) && Number.isFinite(goCompleted) && goCompleted>=goStarted);
assert.deepEqual(origin.source_verification,{
  producer_commit:nativeProducerCommit,retained_source_commit:origin.source_commit,
  upstream_commit:'293893b22a53d8a130a5f7c97cab4bc8e91bcaa4',
  unchanged_files_except_bootstrap_producer:true
});
assert.equal(origin.permissions_regression_fixture.directory_mode,'0700');
assert.equal(origin.permissions_regression_fixture.binary_mode,'0555');
assert.equal(origin.permissions_regression_fixture.historical_mode_not_inferred,true);
assert(Number.isSafeInteger(origin.permissions_regression_fixture.owner_uid) && origin.permissions_regression_fixture.owner_uid>=0);
const oldInputPath = path.join(nativeRoot,'openfga8-retained-verifier-input.json');
const oldInputRecord = await fileRecord(oldInputPath);
assert.deepEqual(oldInputRecord,{bytes:998,sha256:'fcb7a8aa0db087f6489c691d982dd1fb205244e4e99965654fcc3a41e6093e1c'});
assert.deepEqual(origin.verifier_input,oldInputRecord);
assert.deepEqual(origin.members['openfga8-verifier-input.json'],oldInputRecord);
const oldInputs = await readJSON(oldInputPath);
assert.equal(oldInputs.producerSource,origin.source_commit);
assert.equal(oldInputs.workflowCommit,origin.source_workflow_commit);
assert.equal(oldInputs.runId,String(origin.source_run));
assert.equal(oldInputs.runAttempt,'1');
for (const name of inputFiles.filter(name=>name!=='browser-single/bootstrap/build.sh')) {
  assert.equal(oldInputs.sourceSha256[name],inputs.sourceSha256[name],'Retained service/proof source changed: '+name);
}
const retainedFiles = ['LICENSE','SOURCE.txt','build.json','openfga','startup-dial.patch',
  'browser_http_dial.go','browser_http_warmup.go','browser_model_validation.go','browser_model_validation_test.go',
  'browser_model_codec.go','browser_model_codec_test.go','browser_model_codec_integration_test.go',
  'browser_write_typesystem_test.go','browser_resolver_transfer_integration_test.go',
  'dial-tests.log','model-validation-tests.log','write-typesystem-tests.log','model-reuse-race-tests.log'];
assert.deepEqual((await fs.readdir(openfgaDirectory)).sort(),retainedFiles.slice().sort());
assert.deepEqual(Object.keys(origin.members).filter(name=>name.startsWith('browser-openfga/')).sort(),
  retainedFiles.map(name=>'browser-openfga/'+name).sort());
for (const name of retainedFiles) {
  const actual = name==='openfga' ? openfgaBinary : name==='model-reuse-race-tests.log' ? raceLog
    : await fileRecord(path.join(openfgaDirectory,name));
  assert.deepEqual(origin.members['browser-openfga/'+name],actual,'Retained Go output/log changed: '+name);
}
assert.deepEqual(openfgaBinary,{bytes:63066296,sha256:'5204b54bed6c9b74afd2517eb82117b1e91befcee8d1c2c709a554d20ad9ee5f'});
assert.deepEqual(origin.members['browser-openfga/build.json'],
  {bytes:2743,sha256:'d7f903c286657d193eddee5b4bf98ccdc2a56431f1924d7b456555fd506d6315'});
assert.equal(raceLog.sha256,'6ff7585a2ca3f1d6d8df82009e906b3e01b14db23a1296697fba4bb2d5705253');
const retainedOpenfgaOrigin = {
  file:originRecord,sourceRun:origin.source_run,sourceJob:origin.source_job,
  sourceCommit:origin.source_commit,workflowCommit:origin.source_workflow_commit,
  artifact:origin.artifact,archive:origin.archive,goBuildStep:origin.openfga_build_step,
  actualOutputsAndLogsUnchanged:true,historicalDirectoryModeInferred:false
};
const bootstrapDirectory = path.join(source,'artifacts/browser-bootstrap');
const bootstrapBuild = await readJSON(path.join(bootstrapDirectory,'build.json'));
const bootstrapBinary = await fileRecord(path.join(bootstrapDirectory,'bootstrap'));
assert.deepEqual(bootstrapBinary,{bytes:7415568,sha256:'d47425c10ebc1d2c1501e86dd7a1f3e8e5b6d2c9a6e4cf8123f20bd5d8c1866f'});
for (const [key,value] of Object.entries({
  source_commit:'53ff85cfdd3d84c26ae432189790e1e0f72d61db',
  packaging_commit:'d7f7ead003c774e30584798adfde38afb854528d',
  workflow_commit:nativeArchive.head,run_id:String(nativeArchive.run),run_attempt:'1',
  native_tests:'passed',native_harness_runtime:'node',
  expected_openfga_sha_input:'required_verified_build_metadata',browser_acceptance_verified:false,
  binary_bytes:bootstrapBinary.bytes,binary_sha256:bootstrapBinary.sha256,
  openfga_binary_sha256:openfgaBinary.sha256
})) assert.equal(bootstrapBuild[key],value,'Native bootstrap build differs: '+key);
assert.equal(bootstrapBuild.producer_sha256,inputs.sourceSha256['browser-single/bootstrap/build.sh']);
assert.equal(bootstrapBuild.native_harness_sha256,inputs.sourceSha256['browser-single/bootstrap/native-proof.mjs']);
assert.equal(bootstrapBuild.native_test_log_sha256,(await fileRecord(path.join(bootstrapDirectory,'native-tests.log'))).sha256);
const nativeBuildPath = path.join(nativeRoot,'browser-bootstrap-proof/openfga-build.json');
assert.deepEqual(await readJSON(nativeBuildPath),openfga);
assert.equal(bootstrapBuild.openfga_build_sha256,(await fileRecord(nativeBuildPath)).sha256);
assert.equal(bootstrapBuild.openfga_build_sha256,(await fileRecord(path.join(openfgaDirectory,'build.json'))).sha256);
const nativeProofPath = path.join(nativeRoot,'browser-bootstrap-proof/native-result.json');
assert.equal(bootstrapBuild.native_proof_sha256,(await fileRecord(nativeProofPath)).sha256);
const nativeProof = await readJSON(nativeProofPath);
for (const [key,value] of Object.entries({
  passed:true,source_commit:bootstrapBuild.source_commit,packaging_commit:bootstrapBuild.packaging_commit,
  image_id:bootstrapBuild.image_id,binary_sha256:bootstrapBinary.sha256,
  openfga_binary_sha256:openfgaBinary.sha256,browser_acceptance_verified:false,
  opt_in_model_timeout_seconds:30,delayed_model_post_and_get_seconds:6,actual_postgres_model_count:3
})) assert.equal(nativeProof[key],value,'Actual native proof differs: '+key);
assert.equal(nativeProof.native_passes.length,2);
for (const pass of nativeProof.native_passes) {
  assert.equal(pass.accepted,true);
  assert.deepEqual(pass.statuses,[403,204,200,204,403]);
}
assert.deepEqual(nativeProof.default_failures.map(item=>item.phase),['default-post','default-get']);
for (const failure of nativeProof.default_failures) {
  assert.equal(failure.exit,1);
  assert.equal(failure.config_published,false);
  assert.equal(failure.response_still_withheld,true);
  assert(Number.isSafeInteger(failure.elapsed_ms) && failure.elapsed_ms>=4500 && failure.elapsed_ms<6000);
}
const nativeVerification = {
  run:nativeArchive.run,head:nativeArchive.head,producerSource:nativeProducerCommit,
  evidenceArtifact:nativeArchive.artifact,bootstrapArtifact:bootstrapArchive.artifact,
  openfgaBinary,openfgaBuild:await fileRecord(path.join(openfgaDirectory,'build.json')),
  raceLog,retainedOpenfgaOrigin,nativeProof:await fileRecord(nativeProofPath),bootstrapBinary,
  sourceBindings,browserAcceptanceVerified:false
};
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
const kernelConfig = await fs.readFile(path.join(source,'artifacts/browser-kernel/linux.config'),'utf8');
const requiredConfig = ['CONFIG_SYSVIPC=y','CONFIG_IPC_NS=y','CONFIG_SMP=y',
  'CONFIG_X86_LOCAL_APIC=y','CONFIG_X86_IO_APIC=y','CONFIG_HPET_TIMER=y',
  'CONFIG_ACPI=y','CONFIG_X86_PM_TIMER=y','CONFIG_HZ_100=y',
  'CONFIG_X86_MPPARSE=y','# CONFIG_PCI_MSI is not set'];
for (const line of requiredConfig) {
  assert.equal(kernelConfig.split('\n').filter(value=>value===line).length,1,'Required actual kernel config differs: '+line);
}
const originalDirectory = path.join(prepared,'original-runtime');
for (const [name,expected] of Object.entries(originalManifest.files)) {
  assert.deepEqual(await fileRecord(path.join(originalDirectory,name)),expected);
}
// This is the exact SHA-verified converter argument asset, not a VM launch.
const argumentAsset = await fs.readFile(path.join(originalDirectory,'arg-module.js'),'utf8');
const sandbox = {Module:{}};
runInNewContext(argumentAsset,sandbox,{timeout:1000});
const originalArguments = JSON.parse(JSON.stringify(sandbox.Module.arguments));
assert(Array.isArray(originalArguments) && originalArguments.every(value=>typeof value==='string'));
const expectedArguments = originalArguments.slice();
const option = name => {
  assert.equal(expectedArguments.filter(value=>value===name).length,1,'Unexpected argument multiplicity: '+name);
  const index = expectedArguments.indexOf(name);
  assert(index>=0 && index+1<expectedArguments.length);
  return index;
};
assert.equal(expectedArguments[option('-accel')+1],'tcg,tb-size=500,thread=multi');
assert.equal(expectedArguments[option('-m')+1],'128M');
assert.equal(expectedArguments[option('-smp')+1],'1,sockets=1');
expectedArguments.splice(option('-incoming'),2);
expectedArguments[option('-m')+1]='512M';
expectedArguments[option('-smp')+1]='2,sockets=2,cores=1,threads=1';
const append = option('-append');
const flags = expectedArguments[append+1].split(/\s+/);
const separator = flags.indexOf('--');
for (const flag of ['acpi=off','slub_debug=F','loglevel=0']) {
  assert.equal(flags.filter(value=>value===flag).length,1);
  assert(flags.indexOf(flag)<separator);
}
assert(separator>=0 && !expectedArguments.includes('-no-acpi') && !expectedArguments.includes('-no-hpet'));
expectedArguments[append+1]=flags.filter(flag=>flag!=='slub_debug=F')
  .map(flag=>flag==='loglevel=0'?'loglevel=7':flag==='acpi=off'?'acpi=noirq':flag).join(' ');
expectedArguments[option('-netdev')+1]='hubport,id=vmnic,hubid=0';
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
assert.deepEqual(startup.bootstrap_build,bootstrapBuild);
assert.deepEqual(startup.files['runtime/openfga'],openfgaBinary);
assert.deepEqual(startup.files['runtime/bootstrap'],bootstrapBinary);
assert.deepEqual(startup.files['guest.sh'],configurationExperiment.guest);
assert.deepEqual(await fileRecord(path.join(source,'browser-output/wasm/launcher.js')),configurationExperiment.candidateLauncher);
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
const proof = {
  schemaVersion:1,experiment:'firefox_acpi_noirq_boot',diagnosticOnly:true,
  modelExplorerBuildVerified:false,sourceCommit,nativeProducerCommit,sourceFiles,fixtureFiles,configurationExperiment,archives,nativeVerification,
  retainedRuntimeSource:'379e3280c2a1c789071292af141648eb2fc3ffbc',
  originalRuntimeSource:originalManifest.source_commit,
  kernelSource:'af9c5555e312ffaec3785154f5be83be7469c95f',
  rootfsUnchanged:true,preservedSegments,data:expectedData,loader:expectedLoader,
  kernelConfig:kernelFiles['linux.config'],requiredConfig,originalArguments,expectedArguments,
  args:originalManifest.files['arg-module.js'],kernel:kernelFiles.bzImage,emulator:pair,
  startupManifest:await fileRecord(path.join(runtime,'startup-manifest.json')),
  openfgaBuild:openfga,openfgaBinary,bootstrapBuild:startup.bootstrap_build,
  executions:{initialBootstrap:'/tmp/browser-lab-runtime/bootstrap',
    probe:'/usr/local/bin/lab-tool',acceptance:'/usr/local/bin/lab-tool'},
  expectedRuntimeConfiguration:{virtualCPUs:2,memoryMiB:512,accel:'tcg,tb-size=500,thread=multi'},
  authorizationDeadlinesChanged:false,productionAcceptance:false,maxVmStarts:1,hostObservationMilliseconds:300000
};
await fs.writeFile(path.join(evidence,'materialization.json'),JSON.stringify(proof,null,2)+'\n');
console.log('NOIRQ_BOOT_DIAGNOSTIC '+JSON.stringify({phase:'materialized',sourceCommit,nativeProducerCommit,configurationExperiment,
  rootfsUnchanged:true,data:expectedData,loader:expectedLoader,kernel:kernelFiles.bzImage,
  nativeVerification,openfgaBinary,openfgaModelEncoder:openfga.model_json_encoder,
  resolverTypesystemReuse:openfga.first_resolver_typesystem_reuse,
  bootstrapSource:startup.bootstrap_build.source_commit,bootstrapSha256:startup.bootstrap_build.binary_sha256,
  modelExplorerBuildVerified:false}));
