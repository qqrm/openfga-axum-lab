// Diagnostic materialization only. No emulator/service build and no VM startup.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream, promises as fs} from 'node:fs';
import path from 'node:path';
import {spawnSync} from 'node:child_process';

const sourceCommit = 'b04bad4ac109a5fdab185a9025b1ae3982953629';
const kernelSourceCommit = '716b77356dd4aab2747d4e37c5da723f3b7a764f';
const transportSourceCommit = '037a3c22a5fab45a735a6bfea9792c68049a4b62';
const nativeProducerCommit = 'fe42a6bcb91b0702b38836dbec51fa1549cc97be';
const nativeBaseCommit = 'e381339f80dd76777ef526b0073dc5eb34739f26';
const bootstrapCoreCommit = '207d15fab685d3adb9c5d51a048982c2236ae75c';
const source = path.resolve('source-candidate');
const nativeSource = path.resolve('source-native');
const prepared = path.resolve('prepared');
const evidence = path.resolve('artifacts/openfga9-paths-browser-gate');
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
// Five files stay exact to the native deny-probe fixture; the guest's sole
// failure-only syscall addition is checked separately against that fixture.
const fixtureFiles = {
  "browser-single/entrypoint.sh": {
    "gitBlob": "828a107cb517edc1d938a83c996d4017c5506b99",
    "bytes": 5367,
    "mode": "100755"
  },
  "browser-single/pages/guest.sh": {
    "gitBlob": "29d0d5a1794fcc8ce18d231002a6cffd13b130ca",
    "bytes": 6271,
    "mode": "100755"
  },
  "browser-single/pages/startup-payload.mjs": {
    "gitBlob": "65f94eeeb67e0ea0cf0b1769a8967fd79e895d17",
    "bytes": 3580,
    "mode": "100644"
  },
  "browser-single/startup-payload.test.mjs": {
    "gitBlob": "21d5c7151863ab1fa15a6294374bc3612fcdadf3",
    "bytes": 8320,
    "mode": "100644"
  },
  "browser-single/prepare-pages.sh": {
    "gitBlob": "9db9db477749a7d8d62d8cd19eb227bea7b05699",
    "bytes": 9387,
    "mode": "100755"
  },
  "browser-single/check-pages.mjs": {
    "gitBlob": "af096a4ef1bd9045ff2b28d7363d6605ec64bb4e",
    "bytes": 12325,
    "mode": "100644"
  }
};
const uiCommit = 'a438aedc8b3abd7abd1a5038ebeda7e20c63fd70';
assert.equal(command('git',['rev-parse','HEAD^'],{stdio:'pipe'}).trim(),kernelSourceCommit);
assert.equal(command('git',['rev-parse','HEAD^^'],{stdio:'pipe'}).trim(),transportSourceCommit);
assert.equal(command('git',['rev-parse','HEAD^^^'],{stdio:'pipe'}).trim(),uiCommit);
assert.equal(command('git',['rev-parse','HEAD^^^^'],{stdio:'pipe'}).trim(),nativeBaseCommit);
assert.equal(command('git',['rev-parse','HEAD'],{cwd:nativeSource,stdio:'pipe'}).trim(),nativeProducerCommit);
assert.equal(command('git',['rev-parse','HEAD^'],{cwd:nativeSource,stdio:'pipe'}).trim(),nativeBaseCommit);
command('git',['diff','--exit-code','HEAD','--'],{cwd:nativeSource});
const trackedTree = directory => Object.fromEntries(command('git',['ls-tree','-r','-z','HEAD'],
  {cwd:directory,stdio:'pipe'}).split('\0').filter(Boolean).map(row=>{
    const match=/^([0-7]{6}) (blob|commit) ([0-9a-f]{40})\t(.+)$/.exec(row);
    assert(match && ((match[2]==='blob' && ['100644','100755'].includes(match[1])) ||
      (match[2]==='commit' && match[1]==='160000')),'Expected a regular-source blob or gitlink');
    return [match[4],{mode:match[1],type:match[2],git_blob:match[3]}];
  }));
const runtimeTree = trackedTree(source);
const producerTree = trackedTree(nativeSource);
assert.deepEqual(Object.keys(runtimeTree).sort(),Object.keys(producerTree).sort());
for (const tree of [runtimeTree,producerTree]) {
  assert.deepEqual(Object.entries(tree).filter(([,record])=>record.type==='commit'),
    [['vendor/rust-skills',{mode:'160000',type:'commit',git_blob:'fd2a861ab0406a4ac536a55274d14ea6fd1ca9c9'}]]);
}
const reachabilityFiles = {
  'browser-single/openfga/README.md':'2411eb30e6a3b9f1b2e1ef3fd7bf2d88b386897e',
  'browser-single/openfga/browser_model_validation.go':'caf992f9463541cc91d5e747ee00e68a626620b6',
  'browser-single/openfga/browser_model_validation_test.go':'8d3e77fde9b86ec9de0f94c5d51bb143abc63cde',
  'browser-single/openfga/build.sh':'5371a7106196292868da52b3df1476071627ffbc',
  'browser-single/openfga/startup-dial.patch':'3389ef60754e86ac7738f3932bfe7c86b8923562'
};
for (const [directory,parent] of [[source,kernelSourceCommit],[nativeSource,nativeBaseCommit]]) {
  assert.deepEqual(command('git',['diff','--name-only',parent,'HEAD'],{cwd:directory,stdio:'pipe'})
    .trim().split('\n').sort(),Object.keys(reachabilityFiles).sort(),
    'The .9 source changed outside the reviewed schema-reachability adaptation');
}
for (const [name,blob] of Object.entries(reachabilityFiles)) {
  assert.equal(runtimeTree[name].git_blob,blob);
  assert.deepEqual(runtimeTree[name],producerTree[name]);
  assert.deepEqual(await fs.readFile(path.join(source,name)),await fs.readFile(path.join(nativeSource,name)));
}
const changedFiles = {
  "browser-single/pages/log-stream.mjs": {
    "gitBlob": "ef118064b006fbc41a32b98f177ed0f8f2302b4f",
    "mode": "100644",
    "bytes": 8244,
    "sha256": "90941538e1c26c7d657591c840b08f168bdc93db8a56d07921e3a9dc02d31bc3"
  },
  "browser-single/pages/launcher.js": {
    "gitBlob": "918ce05c789b2f2990b6451255934f14de42650e",
    "mode": "100644",
    "bytes": 11765,
    "sha256": "d20a8d95166a2d58d15c69d529b504e608dbfb42705aa6fb51fe23c55af2108d"
  },
  "browser-single/log-stream.test.mjs": {
    "gitBlob": "5287b425a3bd130de6547285e42eca4529ab1281",
    "mode": "100644",
    "bytes": 18004,
    "sha256": "dbe62e63ef170343fdf171e61bc59e18c8b128c4b7a51f4bc724b0a7fa071a52"
  },
  "browser-single/pages/guest.sh": {
    "gitBlob": "a36a387f941571ee322971a4aa92ce24196e03b2",
    "mode": "100755",
    "bytes": 6354,
    "sha256": "213c1e9d0938c5082ffbc6b96086b3d3aa21ab2dc09c58afa1d2b17b1ef80649"
  },
  "browser-single/Kernel.Dockerfile": {
    "gitBlob": "aeb75dc1f17528939559ffe8f8dd15746c0c2a0e",
    "mode": "100644",
    "bytes": 3946,
    "sha256": "9232b9e18ddf0df757fe5b57c6db9c953f1fde06ae68537dfc7d78039454a7a7"
  },
  "browser-single/prepare-kernel.sh": {
    "gitBlob": "224d033e81309c2f40653ea12e4e580f4e3152d7",
    "mode": "100755",
    "bytes": 1253,
    "sha256": "4e572784410d1a70a8f8dcd43a45bdbc3f3e494511f663cf64dec2a6c91e7ead"
  },
  "browser-single/repack-runtime.mjs": {
    "gitBlob": "1f51882a3183bdfd86f0102451706f0daff0d7a2",
    "mode": "100644",
    "bytes": 3975,
    "sha256": "3ccd25511c9bf2d3e824c75eae633a995ff61151522c4923c351094a4226759f"
  }
};
assert.deepEqual(Object.keys(runtimeTree).filter(name=>
  runtimeTree[name].git_blob!==producerTree[name].git_blob || runtimeTree[name].mode!==producerTree[name].mode ||
  runtimeTree[name].type!==producerTree[name].type).sort(),
  Object.keys(changedFiles).sort(),'Native/runtime differences exceed the seven reviewed UI/diagnostic/RNG files');
assert.deepEqual(command('git',['diff','--name-only',transportSourceCommit,kernelSourceCommit],{stdio:'pipe'})
  .trim().split('\n').sort(),['browser-single/Kernel.Dockerfile','browser-single/pages/launcher.js',
    'browser-single/prepare-kernel.sh','browser-single/repack-runtime.mjs'].sort(),
  'RNG source changed outside the four reviewed files');
assert.deepEqual(command('git',['diff','--name-only',uiCommit,transportSourceCommit],{stdio:'pipe'})
  .trim().split('\n'),['browser-single/pages/guest.sh'],'Failure-only diagnostic commit changed other files');
for (const [name,expected] of Object.entries(changedFiles)) {
  assert.equal(command('git',['rev-parse','HEAD:'+name],{stdio:'pipe'}).trim(),expected.gitBlob);
  assert.equal(command('git',['hash-object','--',name],{stdio:'pipe'}).trim(),expected.gitBlob);
  assert.equal(command('git',['ls-tree','HEAD','--',name],{stdio:'pipe'}).split(' ')[0],expected.mode);
  assert.deepEqual(await fileRecord(path.join(source,name)),{bytes:expected.bytes,sha256:expected.sha256});
}
const bytesRecord = bytes => ({bytes:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')});
const originalGuestPath = 'browser-single/pages/guest.sh';
const guest = await fs.readFile(path.join(source,originalGuestPath),'utf8');
const originalGuest = command('git',['show',nativeBaseCommit+':'+originalGuestPath],{stdio:'pipe'});
const guestDiagnosticAddition = "    printf 'syscall: '\n    cat \"$proc/syscall\" 2>/dev/null || true\n    printf '\\n'\n";
assert.equal(guest.split(guestDiagnosticAddition).length,2,'Expected exactly one failure-only syscall snapshot');
assert.deepEqual(Buffer.from(guest.replace(guestDiagnosticAddition,()=>'')),Buffer.from(originalGuest));
assert.deepEqual(bytesRecord(Buffer.from(originalGuest)),
  {bytes:6271,sha256:'4b2eda00f0af736ac02ec4992ea150e9337242512cb3595cbe8a3330609010ab'});
assert.equal(command('git',['rev-parse',nativeBaseCommit+':'+originalGuestPath],{stdio:'pipe'}).trim(),
  fixtureFiles[originalGuestPath].gitBlob);
const launcherPath = 'browser-single/pages/launcher.js';
const expectedLauncher = {
  gitBlob:'30c578771b85f7eb5f5adc15f94fd72142c21c7d',mode:'100644',
  bytes:10715,sha256:'055204abad48a78914c7322b9692b972f68fc06522578d7994f3b3fa38d83bd0'
};
const launcher = await fs.readFile(path.join(source,launcherPath),'utf8');
const rngLauncherAddition = "    // Emscripten's host /dev/urandom reads Web Crypto bytes. Linux's VirtIO\n    // RNG driver credits them; getrandom retains its normal readiness checks.\n    if (typeof globalThis.crypto?.getRandomValues !== 'function') throw new Error('Web Crypto is required for guest entropy');\n    if (args.some(arg => typeof arg === 'string' &&\n        (arg === '-seed' || arg.startsWith('-seed=') || arg.includes('browser_rng') ||\n         /^(?:rng-(?:random|builtin|egd)|virtio-rng)/.test(arg)))) {\n      throw new Error('Unexpected existing or deterministic guest entropy configuration');\n    }\n    args.push('-object', 'rng-random,filename=/dev/urandom,id=browser_rng',\n      '-device', 'virtio-rng-pci,rng=browser_rng');\n    window.labEvidence.vm.entropyConfiguration = {backend:'rng-random',hostDevice:'/dev/urandom',guestDevice:'virtio-rng-pci',webCryptoRequired:true};\n";
assert.equal(launcher.split(rngLauncherAddition).length,2,'Expected exactly one reviewed RNG addition');
const transportLauncher = launcher.replace(rngLauncherAddition,()=> '');
assert.deepEqual(Buffer.from(transportLauncher),
  Buffer.from(command('git',['show',transportSourceCommit+':'+launcherPath],{stdio:'pipe'})),
  'Removing only the RNG addition must restore the exact browser-tested launcher');
assert.deepEqual(bytesRecord(Buffer.from(transportLauncher)),
  {bytes:10898,sha256:'e369bc067e1bcc1f932d7aeedffa1cb34bb63903093501791a722f88577343ee'});
const entropyConfiguration = {backend:'rng-random',hostDevice:'/dev/urandom',
  guestDevice:'virtio-rng-pci',webCryptoRequired:true};
const originalLauncher = command('git',['show',nativeBaseCommit+':'+launcherPath],{stdio:'pipe'});
assert.equal(command('git',['rev-parse',nativeBaseCommit+':'+launcherPath],{stdio:'pipe'}).trim(),expectedLauncher.gitBlob);
assert.deepEqual(bytesRecord(Buffer.from(originalLauncher)),{bytes:expectedLauncher.bytes,sha256:expectedLauncher.sha256});
const launcherChanges = [
  ["import {createTranscriptLog, createCoalescedTranscriptRender, guestLogEvent} from './log-stream.mjs';",
   "import {createTranscriptLog, guestLogEvent} from './log-stream.mjs';"],
  ["const transcriptRender = createCoalescedTranscriptRender(text => { element('log').textContent = text; }, fail);\nwindow.__flushBrowserLabTranscript = transcriptRender.flush;\nconst log = createTranscriptLog(transcriptRender.render, chunk => {",
   "const log = createTranscriptLog(text => { element('log').textContent = text; }, chunk => {"]
];
let restoredLauncher = transportLauncher;
for (const [candidate,original] of launcherChanges) {
  assert.equal(restoredLauncher.split(candidate).length,2,'Expected exactly one reviewed launcher change');
  restoredLauncher = restoredLauncher.replace(candidate,()=>original);
}
assert.deepEqual(Buffer.from(restoredLauncher),Buffer.from(originalLauncher),'Launcher changed outside render coalescing and the exact RNG addition');
const launcherRecord = await fileRecord(path.join(source,launcherPath));
for (const [name,expected] of Object.entries(fixtureFiles)) {
  if (name===originalGuestPath) continue;
  assert.equal(command('git',['hash-object','--',name],{stdio:'pipe'}).trim(),expected.gitBlob,
    'Reviewed production runtime file changed: '+name);
  assert.equal((await fs.stat(path.join(source,name))).size,expected.bytes);
  assert.equal(command('git',['ls-tree','HEAD','--',name],{stdio:'pipe'}).split(' ')[0],expected.mode);
}

const bootstrapSource = path.join(source,'browser-bootstrap-source');
assert.equal(command('git',['-C',bootstrapSource,'rev-parse','HEAD'],{stdio:'pipe'}).trim(),bootstrapCoreCommit);
command('git',['-C',bootstrapSource,'diff','--exit-code','HEAD','--']);
for (const [name,blob] of Object.entries({
  'tools/bootstrap.rs':'a38ba1e8f792e7c0ed019c92f0ffd08a5d74a7a3',
  'tools/bootstrap_tests.rs':'f5b5e56b04f9d8dd518228f84b7470b9949c65ca'
})) assert.equal(command('git',['hash-object','--',name],{stdio:'pipe'}).trim(),blob);
const configurationExperiment = {
  bootstrap:'one_real_higher_consistency_ownership_deny',kernelIRQ:'default_acpi',
  runtimeSource:sourceCommit,nativeProducerCommit,nativeBaseCommit,bootstrapCoreCommit,
  schemaReachabilityFiles:reachabilityFiles,nativeRuntimeDifferencePathsVerified:true,
  runtimeSourceMatchesNativeProducer:false,launcherIdentityVerified:true,
  launcher:launcherRecord,launcherGitBlob:changedFiles[launcherPath].gitBlob,launcherMode:expectedLauncher.mode,
  nativeLauncher:{bytes:expectedLauncher.bytes,sha256:expectedLauncher.sha256},
  guest:await fileRecord(path.join(source,originalGuestPath)),
  nativeGuest:bytesRecord(Buffer.from(originalGuest)),
  reviewedChangedFiles:changedFiles,guestDiagnosticInverseVerified:true,launcherInverseVerified:true,
  launcherRngInverseVerified:true,transportSourceCommit,
  testedLauncher:bytesRecord(Buffer.from(transportLauncher)),entropyConfiguration,
  entropyDeliveryVerified:false,
  renderDelayMilliseconds:16,partialBatchDelayMilliseconds:16,
  synchronousProtocolUnchanged:true,unchangedAuthorizationDeadlines:true,unchangedModelAndRootfs:true,
  scope:'Fresh native-tested .9 schema reachability; seven reviewed UI/diagnostic/RNG differences remain separate from native producer fe42'
};

const archives = {};
for (const label of ['retained','kernel','native','bootstrap','lightweight']) {
  archives[label] = await readJSON(path.join(prepared,label+'-archive-provenance.json'));
}
const lightRoot = path.join(prepared,'lightweight-evidence');
const lightPrefix = 'artifacts/browser-log-transport-candidate/';
const lightInputs = ['browser-single/pages/log-stream.mjs','browser-single/pages/launcher.js',
  'browser-single/log-stream.test.mjs','browser-single/runtime-errors.test.mjs',
  'browser-single/acceptance-protocol.test.mjs','e2e/package.json','e2e/package-lock.json'];
const lightMembers = [lightPrefix+'summary.json',lightPrefix+'node-tests.log',
  'diagnostics/check-log-transport-candidate.mjs',
  ...lightInputs.slice(0,3).map(name=>'source-candidate/'+name)];
const lightArchive = archives.lightweight;
assert.equal(lightArchive.head,'0a8d7dc4ba611fe7071b34005410f4dc832d7b71');
assert.equal(lightArchive.artifact_name,'browser-log-transport-candidate');
assert(Number.isSafeInteger(lightArchive.run) && lightArchive.run>0);
assert(Number.isSafeInteger(lightArchive.artifact) && lightArchive.artifact>0);
assert.deepEqual(Object.keys(lightArchive.members).sort(),lightMembers.slice().sort());
for (const name of lightMembers) {
  assert.deepEqual(await fileRecord(path.join(lightRoot,name)),lightArchive.members[name],
    'Lightweight proof member changed: '+name);
}
const light = await readJSON(path.join(lightRoot,lightPrefix+'summary.json'));
for (const [key,value] of Object.entries({
  schemaVersion:1,sourceCommit:transportSourceCommit,productionAcceptance:false,vmStarts:0,runtimeDownloads:0,
  expectedCases:14,completedCases:14,passed:true
})) assert.equal(light[key],value,'Lightweight proof differs: '+key);
assert.deepEqual(light.failures,[]);
assert.deepEqual(light.unexpectedRequests,[]);
assert.deepEqual(Object.keys(light.inputs).sort(),lightInputs.slice().sort());
for (const name of lightInputs) {
  // The only changed lightweight input is launcher: its exact RNG inverse
  // must equal the previously tested bytes. Every other input is unchanged.
  const actual = name===launcherPath ? bytesRecord(Buffer.from(transportLauncher))
    : await fileRecord(path.join(source,name));
  assert.deepEqual(light.inputs[name],actual,'Transport proof input differs: '+name);
  if (lightInputs.indexOf(name)<3) {
    assert.deepEqual(await fileRecord(path.join(lightRoot,'source-candidate',name)),actual);
  }
}
assert.deepEqual(lightArchive.members['diagnostics/check-log-transport-candidate.mjs'],
  {bytes:16939,sha256:'6042f8049571a33e53f34defc36037d580aa797127397cd28ca2373a4bf92350'});
assert.equal(light.harnessSha256,lightArchive.members['diagnostics/check-log-transport-candidate.mjs'].sha256);
const lightScenarios = ['synchronous_uart_burst','separate_tasks_ack_gate','separate_tasks_unblocked',
  'finish_partial_utf16','render_error_at_cutoff','delivery_error','pending_chunk_bound'];
assert.equal(light.cases.length,14);
assert.deepEqual(light.cases.map(item=>item.engine+'/'+item.scenario).sort(),
  ['chromium','firefox'].flatMap(engine=>lightScenarios.map(name=>engine+'/'+name)).sort());
for (const item of light.cases) {
  const expectedFailure = ['render_error_at_cutoff','delivery_error','pending_chunk_bound'].includes(item.scenario);
  assert.equal(item.passed,true);
  assert.equal(item.expectedFailure,expectedFailure);
  assert.equal(item.captureQualified,!expectedFailure);
  assert.equal(item.maximumInFlight,1);
  assert(Number.isSafeInteger(item.maximumBatch) && item.maximumBatch>0 && item.maximumBatch<=1024);
  assert(Number.isSafeInteger(item.bindingCalls) && item.bindingCalls>0);
}
const nodeLog = await fs.readFile(path.join(lightRoot,lightPrefix+'node-tests.log'),'utf8');
const nodeCounts = Object.fromEntries([...nodeLog.matchAll(/^# (tests|pass|fail|cancelled|skipped|todo) (\d+)\r?$/gm)]
  .map(match=>[match[1],Number(match[2])]));
assert(Number.isSafeInteger(nodeCounts.tests) && nodeCounts.tests>=22);
assert.equal(nodeCounts.pass,nodeCounts.tests);
for (const name of ['fail','cancelled','skipped','todo']) assert.equal(nodeCounts[name],0);
const lightweightVerification = {
  run:lightArchive.run,head:lightArchive.head,artifact:lightArchive.artifact,
  archive:lightArchive.archive,sourceCommit:transportSourceCommit,appliedRuntimeSource:sourceCommit,
  launcherRngInverseVerified:true,passed:true,cases:light.cases,nodeCounts,
  memberRecords:lightArchive.members,productionAcceptance:false,vmStarts:0,runtimeDownloads:0
};
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
// The retained .7 model/emulator provenance is separate from the fresh native-tested .9 binary.
const nativeArchive = archives.native;
const bootstrapArchive = archives.bootstrap;
assert(Number.isSafeInteger(nativeArchive.run) && nativeArchive.run>0);
assert.match(nativeArchive.head,/^[0-9a-f]{40}$/);
assert.equal(nativeArchive.run,43478795296);
assert.equal(nativeArchive.head,'16266f3990af32a375039ce945322c26fa44b58c');
assert.equal(nativeArchive.artifact,14065112277);
assert.deepEqual(nativeArchive.archive,{"bytes":24001397,"sha256":"a34151ad1bac812410c57d42ee93b50c5f50ca96c70299b43431ab9539bcca61"});
assert.equal(bootstrapArchive.artifact,14065566791);
assert.deepEqual(bootstrapArchive.archive,{"bytes":3321947,"sha256":"ae6ff35cec4281e13343f9f2b9def30200a803fcdaba0380e816e55b5402ab19"});
assert.equal(nativeArchive.run,bootstrapArchive.run);
assert.equal(nativeArchive.head,bootstrapArchive.head);
assert.notEqual(nativeArchive.artifact,bootstrapArchive.artifact);
assert.equal(nativeArchive.artifact_name,'browser-openfga9-paths-native-evidence');
assert.equal(bootstrapArchive.artifact_name,'browser-bootstrap-openfga9-paths');
const nativeRoot = path.join(prepared,'native-evidence');
const inputs = await readJSON(path.join(nativeRoot,'openfga9-verifier-input.json'));
for (const [key,value] of Object.entries({
  schemaVersion:3,experiment:'fresh_openfga9_schema_reachability_and_bootstrap_deny',
  runtimeSource:sourceCommit,openfgaInputScope:'exact_openfga_build_inputs_and_tests',
  runtimeOpenfgaInputsMatchProducer:true,otherRuntimeFilesCompared:false,
  producerSource:nativeProducerCommit,rustSource:'207d15fab685d3adb9c5d51a048982c2236ae75c',
  packagingSource:'4b7a1e1933083232fb8305a60a47bdd3c32e6689',
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
  schema_reachability_reuse:'readonly_plain_user_paths_on_startup_typesystem',
  schema_reachability_max_relations:512,schema_reachability_tests:'passed',
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
// This proof is a fresh .9 Go build, not retained .8 output.
assert.equal(Object.hasOwn(inputs,'retainedOpenfgaOriginSha256'),false);
assert.deepEqual(inputs.excludedDocumentation,['browser-single/openfga/README.md']);
const openfgaInputPaths = [".dockerignore","browser-single/OpenFGA.Dockerfile","browser-single/openfga/browser_http_dial.go","browser-single/openfga/browser_http_dial_test.go","browser-single/openfga/browser_http_warmup.go","browser-single/openfga/browser_http_warmup_test.go","browser-single/openfga/browser_model_codec.go","browser-single/openfga/browser_model_codec_integration_test.go","browser-single/openfga/browser_model_codec_test.go","browser-single/openfga/browser_model_validation.go","browser-single/openfga/browser_model_validation_test.go","browser-single/openfga/browser_resolver_transfer_integration_test.go","browser-single/openfga/browser_write_typesystem_test.go","browser-single/openfga/build.sh","browser-single/openfga/startup-dial.patch","browser-single/openfga/upstream.sha256","fga/model.json"];
const relevantInput = name => name === '.dockerignore' || name === 'fga/model.json' ||
  name === 'browser-single/OpenFGA.Dockerfile' || name === 'browser-single/OpenFGA.Dockerfile.dockerignore' ||
  (name.startsWith('browser-single/openfga/') && name !== 'browser-single/openfga/README.md');
const regularInputs = tree => Object.fromEntries(Object.entries(tree).filter(([name])=>relevantInput(name))
  .map(([name,record])=>{
    assert.equal(record.type,'blob','Expected a regular OpenFGA source input: '+name);
    assert(['100644','100755'].includes(record.mode),'Unexpected OpenFGA source mode: '+name);
    return [name,{mode:record.mode,git_blob:record.git_blob}];
  }));
const listedInputs = regularInputs(runtimeTree);
const producerInputs = regularInputs(producerTree);
assert.deepEqual(Object.keys(listedInputs).sort(),openfgaInputPaths.slice().sort());
assert.deepEqual(listedInputs,producerInputs,'Fresh native/runtime OpenFGA source identities differ');
const verifiedInputs = {};
for (const name of openfgaInputPaths) {
  const runtimeRecord = await fileRecord(path.join(source,name));
  assert.deepEqual(await fileRecord(path.join(nativeSource,name)),runtimeRecord);
  verifiedInputs[name] = {...listedInputs[name],...runtimeRecord};
}
const inputSetSha256 = createHash('sha256').update(JSON.stringify(verifiedInputs)).digest('hex');
assert.deepEqual(inputs.verifiedInputs,verifiedInputs);
assert.equal(inputs.inputSetSha256,inputSetSha256);
assert.equal(inputSetSha256,'eea1a1b6545355da366fab9c1d7de3d3bb71f8a2b7581d3db3c4c49be9aa0634');
const verifierInputRecord = await fileRecord(path.join(nativeRoot,'openfga9-verifier-input.json'));
const freshVerificationPath = path.join(nativeRoot,'openfga9-build-verification.json');
const freshVerification = await readJSON(freshVerificationPath);
assert.equal((await fileRecord(freshVerificationPath)).sha256,'54ab092ecfc0accaff03323fb43e64e462ea355eeb269d4bdb3c247337042cef');
for (const [key,value] of Object.entries({
  schemaVersion:1,kind:'fresh_go_build',producerSource:nativeProducerCommit,runtimeSource:sourceCommit,
  workflowCommit:nativeArchive.head,runId:String(nativeArchive.run),runAttempt:'1',
  inputRecordSha256:verifierInputRecord.sha256,inputSetSha256,
  directoryPermissionFixture:'0700',browserAcceptanceVerified:false
})) assert.equal(freshVerification[key],value,'Fresh Go verification differs: '+key);
const freshFiles = ['LICENSE','SOURCE.txt','build.json','openfga','startup-dial.patch',
  'browser_http_dial.go','browser_http_warmup.go','browser_model_validation.go','browser_model_validation_test.go',
  'browser_model_codec.go','browser_model_codec_test.go','browser_model_codec_integration_test.go',
  'browser_write_typesystem_test.go','browser_resolver_transfer_integration_test.go',
  'dial-tests.log','model-validation-tests.log','write-typesystem-tests.log','model-reuse-race-tests.log'];
assert.deepEqual((await fs.readdir(openfgaDirectory)).sort(),freshFiles.slice().sort());
assert.deepEqual(Object.keys(freshVerification.members).sort(),freshFiles.slice().sort());
assert.deepEqual(Object.keys(nativeArchive.members).filter(name=>name.startsWith('browser-openfga/')).sort(),
  freshFiles.map(name=>'browser-openfga/'+name).sort());
for (const name of freshFiles) {
  const actual = name==='openfga' ? openfgaBinary : name==='model-reuse-race-tests.log' ? raceLog
    : await fileRecord(path.join(openfgaDirectory,name));
  assert(actual.bytes>0);
  assert.deepEqual(freshVerification.members[name],actual,'Fresh Go output/log changed: '+name);
  assert.deepEqual(nativeArchive.members['browser-openfga/'+name],actual);
}
const expectedFreshOpenfga = {"bytes":63074488,"sha256":"7841a91b524fc1be097f2f816a8ed39f502e1372bbf96300dc9008b3336f98c3"};
assert(expectedFreshOpenfga,'The fresh .9 binary pin has not been reviewed');
assert.deepEqual(openfgaBinary,expectedFreshOpenfga);
assert.deepEqual(freshVerification.members['build.json'],{"bytes":2910,"sha256":"d430aad43f10805ee655b8180fd2c25cc0a65eaaff1da57b37d3f556ad19f983"});
assert.deepEqual(raceLog,{"bytes":37910,"sha256":"57d2b1550eb49cf2cd0d2113277dae1dec0750eee888b4a68bc0c059d9991a82"});
assert.notEqual(openfgaBinary.sha256,'5204b54bed6c9b74afd2517eb82117b1e91befcee8d1c2c709a554d20ad9ee5f',
  'Retained .8 output cannot be used as the fresh .9 binary');
assert((await fs.readFile(path.join(openfgaDirectory,'SOURCE.txt'),'utf8'))
  .includes('Browser build v1.10.3-browser-startup.9 '));
const goBuildLog = await fileRecord(path.join(nativeRoot,'browser-openfga-build.log'));
assert(goBuildLog.bytes>0 && goBuildLog.bytes<=64*1024*1024);
assert.equal(freshVerification.buildLogSha256,goBuildLog.sha256);
assert.deepEqual(nativeArchive.members['browser-openfga-build.log'],goBuildLog);
const pathTests = ['TestBrowserModelUserPathsPinnedParity','TestBrowserModelUserPathsEligibilityAndBound',
  'TestBrowserModelUserPathsFailureDoesNotPublish','TestBrowserModelUserPathsConcurrentReadonly'];
assert.deepEqual(freshVerification.completedPathTests,pathTests);
for (const name of ['model-validation-tests.log','model-reuse-race-tests.log']) {
  const log = await fs.readFile(path.join(openfgaDirectory,name),'utf8');
  assert(!/^(?:--- FAIL:|FAIL(?:\s|$))/m.test(log),'Go failure in '+name);
  for (const test of pathTests) assert(new RegExp('^--- PASS: '+test+' \\(','m').test(log),
    'Missing completed .9 test in '+name+': '+test);
  assert(/^ok\s+github\.com\/openfga\/openfga\/pkg\/typesystem\s+/m.test(log));
}
for (const [name,packages] of [
  ['dial-tests.log',['cmd/run']],
  ['write-typesystem-tests.log',['pkg/server/commands','pkg/server']],
  ['model-reuse-race-tests.log',['cmd/run','pkg/typesystem','pkg/server']]
]) {
  const log = await fs.readFile(path.join(openfgaDirectory,name),'utf8');
  assert(!/^(?:--- FAIL:|FAIL(?:\s|$))/m.test(log),'Go failure in '+name);
  for (const pkg of packages) assert(new RegExp('^ok\\s+github\\.com/openfga/openfga/'+pkg+'\\s+','m').test(log),
    'Existing Go regression package did not pass: '+name+' '+pkg);
}
const job = nativeArchive.native_job;
assert.equal(nativeArchive.native_run_attempt,1);
for (const [key,value] of Object.entries({
  id:132069573580,run_id:nativeArchive.run,head_sha:nativeArchive.head,
  name:'native',status:'completed',conclusion:'success'
})) assert.equal(job[key],value,'Fresh native job identity differs: '+key);
const requiredNativeSteps = [
  'Check source identity, repository contract and both producer scripts',
  'Build and regression-test the fresh OpenFGA .9 producer',
  'Bind the fresh binary to exact inputs and actual regular and race tests',
  'Execute the versioned native bootstrap producer with this exact OpenFGA output',
  'Verify all four outputs and their real native OpenFGA provenance'
];
for (const name of requiredNativeSteps) {
  const matches = job.steps.filter(step=>step.name===name);
  assert.equal(matches.length,1,'Missing exact native proof step: '+name);
  assert.equal(matches[0].status,'completed');
  assert.equal(matches[0].conclusion,'success');
}
const freshGoBuild = {
  input:verifierInputRecord,verification:await fileRecord(freshVerificationPath),
  kind:freshVerification.kind,producerSource:nativeProducerCommit,runtimeSource:sourceCommit,
  run:nativeArchive.run,head:nativeArchive.head,job:job.id,
  inputSetSha256,verifiedInputs,members:freshVerification.members,buildLog:goBuildLog,
  completedPathTests:pathTests,directoryPermissionFixture:'0700',
  existingGoRegressionPackagesPassed:true,browserAcceptanceVerified:false
};
const bootstrapDirectory = path.join(source,'artifacts/browser-bootstrap');
const bootstrapBuild = await readJSON(path.join(bootstrapDirectory,'build.json'));
const bootstrapBinary = await fileRecord(path.join(bootstrapDirectory,'bootstrap'));
const expectedFreshBootstrap = {"bytes":7444640,"sha256":"e1143b7d5f8cc564e09d12c8f685bd7cf33bd0beb1505f2b7cfc1f6f53e1d275"};
assert(expectedFreshBootstrap,'The newly proved bootstrap binary pin has not been reviewed');
assert.deepEqual(bootstrapBinary,expectedFreshBootstrap);
for (const [key,value] of Object.entries({
  source_commit:'207d15fab685d3adb9c5d51a048982c2236ae75c',
  packaging_commit:'4b7a1e1933083232fb8305a60a47bdd3c32e6689',
  workflow_commit:nativeArchive.head,run_id:String(nativeArchive.run),run_attempt:'1',
  native_tests:'passed',native_harness_runtime:'node',
  empty_store_check:'one_real_higher_consistency_ownership_deny',
  empty_store_check_default:false,browser_empty_store_check:true,
  empty_store_check_timeout_seconds:5,empty_store_check_tests:'passed',
  expected_openfga_sha_input:'required_verified_build_metadata',browser_acceptance_verified:false,
  binary_bytes:bootstrapBinary.bytes,binary_sha256:bootstrapBinary.sha256,
  openfga_binary_sha256:openfgaBinary.sha256
})) assert.equal(bootstrapBuild[key],value,'Native bootstrap build differs: '+key);
assert.equal(bootstrapBuild.bootstrap_source_sha256,(await fileRecord(path.join(bootstrapSource,'tools/bootstrap.rs'))).sha256);
assert.equal(bootstrapBuild.bootstrap_tests_sha256,(await fileRecord(path.join(bootstrapSource,'tools/bootstrap_tests.rs'))).sha256);
assert.equal(bootstrapBuild.producer_sha256,inputs.sourceSha256['browser-single/bootstrap/build.sh']);
assert.equal(bootstrapBuild.native_harness_sha256,inputs.sourceSha256['browser-single/bootstrap/native-proof.mjs']);
assert.equal(bootstrapBuild.native_test_log_sha256,(await fileRecord(path.join(bootstrapDirectory,'native-tests.log'))).sha256);
const nativeBuildPath = path.join(nativeRoot,'browser-bootstrap-proof/openfga-build.json');
assert.deepEqual(await readJSON(nativeBuildPath),openfga);
assert.equal(bootstrapBuild.openfga_build_sha256,(await fileRecord(nativeBuildPath)).sha256);
assert.equal(bootstrapBuild.openfga_build_sha256,(await fileRecord(path.join(openfgaDirectory,'build.json'))).sha256);
const nativeProofPath = path.join(nativeRoot,'browser-bootstrap-proof/native-result.json');
assert.equal(bootstrapBuild.native_proof_sha256,(await fileRecord(nativeProofPath)).sha256);
assert.equal(bootstrapBuild.native_proof_sha256,'fa63509dedf0298f8e95595451f2662f5d51118e57f81e33889bd0afbbd2d314');
assert.equal(bootstrapBuild.native_test_log_sha256,'ada7f5c656c4d119199787abd5152d1dd3e35b5bf0ca2290127dfa975efc82c3');
const nativeProof = await readJSON(nativeProofPath);
for (const [key,value] of Object.entries({
  passed:true,source_commit:bootstrapBuild.source_commit,packaging_commit:bootstrapBuild.packaging_commit,
  image_id:bootstrapBuild.image_id,binary_sha256:bootstrapBinary.sha256,
  openfga_binary_sha256:openfgaBinary.sha256,browser_acceptance_verified:false,
  opt_in_model_timeout_seconds:30,delayed_model_post_and_get_seconds:6,actual_postgres_model_count:4,
  delayed_empty_store_check_seconds:6,actual_postgres_tuple_count_before_app:0
})) assert.equal(nativeProof[key],value,'Actual native proof differs: '+key);
assert.equal(nativeProof.native_passes.length,2);
for (const pass of nativeProof.native_passes) {
  assert.equal(pass.accepted,true);
  assert.deepEqual(pass.statuses,[403,204,200,204,403]);
}
assert.deepEqual(nativeProof.default_failures.map(item=>item.phase),['default-post','default-get']);
for (const failure of [...nativeProof.default_failures,nativeProof.empty_store_check_deadline_failure]) {
  assert.equal(failure.exit,1);
  assert.equal(failure.config_published,false);
  assert.equal(failure.response_still_withheld,true);
  assert(Number.isSafeInteger(failure.elapsed_ms) && failure.elapsed_ms>=4500 && failure.elapsed_ms<6000);
}

assert.equal(nativeProof.empty_store_check_deadline_failure.phase,'check-deadline');
for (const check of [nativeProof.empty_store_check,nativeProof.empty_store_check_deadline_failure.real_check]) {
  assert.equal(check.request_count,1);
  assert.equal(check.status,200);
  assert.equal(check.allowed,false);
  assert.equal(check.consistency,'HIGHER_CONSISTENCY');
  assert.equal(check.tuple_count_before,0);
  assert.match(check.store_id,/^[0-9A-HJKMNP-TV-Z]{26}$/);
  assert.match(check.authorization_model_id,/^[0-9A-HJKMNP-TV-Z]{26}$/);
  assert.deepEqual(check.tuple_key,{
    user:'user:browser-bootstrap-probe',relation:'ownership',
    object:'lakekeeper_table:browser-bootstrap-probe'
  });
}
assert.notEqual(nativeProof.empty_store_check.store_id,
  nativeProof.empty_store_check_deadline_failure.real_check.store_id);
assert.notEqual(nativeProof.empty_store_check.authorization_model_id,
  nativeProof.empty_store_check_deadline_failure.real_check.authorization_model_id);

const nativeVerification = {
  run:nativeArchive.run,head:nativeArchive.head,producerSource:nativeProducerCommit,
  evidenceArtifact:nativeArchive.artifact,bootstrapArtifact:bootstrapArchive.artifact,
  openfgaBinary,openfgaBuild:await fileRecord(path.join(openfgaDirectory,'build.json')),
  raceLog,freshGoBuild,nativeProof:await fileRecord(nativeProofPath),bootstrapBinary,
  sourceBindings,emptyStoreCheck:nativeProof.empty_store_check,
  emptyStoreCheckDeadlineFailure:nativeProof.empty_store_check_deadline_failure,
  postgresModelCount:4,postgresTupleCountBeforeApp:0,browserAcceptanceVerified:false
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
  "COPYING": {
    "bytes": 496,
    "sha256": "fb5a425bd3b3cd6071a3a9aff9909a859e7c1158d54d32e07658398cd67eb6a0"
  },
  "GPL-2.0": {
    "bytes": 18729,
    "sha256": "f6b78c087c3ebdf0f3c13415070dd480a3f35d8fc76f3d02180a407c1c812f79"
  },
  "SOURCE.txt": {
    "bytes": 443,
    "sha256": "9893a165618938a167fd389067e6140c273c1878fde681131ef8bd2675396f62"
  },
  "acpi-baseline.config": {
    "bytes": 75338,
    "sha256": "fbee1ad0bc5fb73f2ca82ba98fa045b76ff19e67a148b40cb93b63916c2ad9b8"
  },
  "acpi.config.diff": {
    "bytes": 1707,
    "sha256": "84520b160eab472b1db6d6b028bd3f0bae19dfda5498d0d914699d514dfd388c"
  },
  "build.sha256": {
    "bytes": 521,
    "sha256": "28ffe830b37725cdbcd5618fdae6009f7603f3272c2a503dc87eef19a3bf7067"
  },
  "bzImage": {
    "bytes": 3629632,
    "sha256": "17abd1165f3b228457d809adf5f4eb9163c282b6ab5b4828e1c9803a257e5421"
  },
  "ipc-baseline.config": {
    "bytes": 72396,
    "sha256": "b109d75227af31a6128280fd192bc06aa1e3d290075a2cf1c2e4fe148df00fcc"
  },
  "linux.config": {
    "bytes": 75327,
    "sha256": "4ba9caa843f2e00738b1892a3d5cb5ce8e976debdaa3629ce584614bd8f9345d"
  },
  "rng.config.diff": {
    "bytes": 25,
    "sha256": "c538bd039d205bd0951c068607f8240cf3baf31d69d744434a802cd9e5876358"
  },
  "upstream.config": {
    "bytes": 72360,
    "sha256": "000b13b14f4ae8f4c2ef6f7a8b5df126d7557ee2cc804e5ed620d4ffdd10bd92"
  }
};
const kernelDirectory = path.join(source,'artifacts/browser-kernel');
const kernelArchive = archives.kernel;
assert.equal(kernelArchive.run,43473084506);
assert.equal(kernelArchive.head,'84af5832aed519e7f0628e36afd8d2fc3e4cc270');
assert.equal(kernelArchive.artifact,14061334425);
assert.equal(kernelArchive.artifact_name,'browser-virtio-rng-kernel-candidate');
assert.deepEqual(kernelArchive.archive,
  {bytes:3689588,sha256:'c71c1e7fdb705e1b5351ed82d119322c0ff7ff9f9f068d5a75a611aded2935ad'});
const kernelBuild = await readJSON(path.join(kernelDirectory,'metadata.json'));
for (const [key,value] of Object.entries({
  source_commit:kernelSourceCommit,workflow_commit:kernelArchive.head,run_id:String(kernelArchive.run),run_attempt:'1',
  recipe_git_blob:changedFiles['browser-single/Kernel.Dockerfile'].gitBlob,
  linux_source:'830b3c68c1fb1e9176028d02ef86f3cf76aa2476',
  upstream_config_blob:'48dd1bf6191dd8d0d220358167f52dce36c69c45',
  source_checks_outcome:'success',build_outcome:'success',kernel_candidate_built:true,
  baseline_matches_previous_kernel_config:true,exactly_one_config_change:true,
  exact_rng_config_diff:true,previous_acpi_diff_preserved:true,kernel_hash_manifest_matches:true,
  complete_diff_within_output_budget:true,entropy_delivery_verified:false,browser_acceptance_verified:false
})) assert.equal(kernelBuild[key],value,'Kernel producer metadata differs: '+key);
assert.deepEqual(kernelBuild.missing_files,[]);
assert.deepEqual(kernelBuild.missing_config,[]);
assert.equal((await fs.readFile(path.join(kernelDirectory,'source-commit.txt'),'utf8')).trim(),kernelSourceCommit);
assert.equal((await fs.readFile(path.join(kernelDirectory,'workflow-commit.txt'),'utf8')).trim(),kernelArchive.head);
const recipeRecord = await fileRecord(path.join(source,'browser-single/Kernel.Dockerfile'));
assert.deepEqual(await fileRecord(path.join(kernelDirectory,'Kernel.Dockerfile')),recipeRecord);
assert.deepEqual(kernelBuild.files['Kernel.Dockerfile'],recipeRecord);
assert.deepEqual(Object.keys(kernelArchive.members).sort(),
  [...Object.keys(kernelFiles).map(name=>'kernel/'+name),
    'metadata.json','Kernel.Dockerfile','source-commit.txt','workflow-commit.txt'].sort());
for (const [name,expected] of Object.entries(kernelFiles)) {
  assert.deepEqual(await fileRecord(path.join(kernelDirectory,name)),expected,'Pinned kernel member changed: '+name);
  assert.deepEqual(kernelArchive.members['kernel/'+name],expected);
  assert.deepEqual(kernelBuild.files['kernel/'+name],expected);
}
const kernelConfigBytes = await fs.readFile(path.join(kernelDirectory,'linux.config'));
const baselineBytes = await fs.readFile(path.join(kernelDirectory,'acpi-baseline.config'));
const baselineText = baselineBytes.toString('utf8');
const oldFlag = '# CONFIG_HW_RANDOM_VIRTIO is not set\n';
const newFlag = 'CONFIG_HW_RANDOM_VIRTIO=y\n';
assert.equal(baselineText.split(oldFlag).length,2,'Expected one disabled RNG driver in pinned ACPI baseline');
assert.deepEqual(Buffer.from(baselineText.replace(oldFlag,()=>newFlag)),kernelConfigBytes,
  'Kernel configuration changed beyond enabling the VirtIO RNG driver');
assert.deepEqual(await fs.readFile(path.join(kernelDirectory,'rng.config.diff')),
  Buffer.from(' HW_RANDOM_VIRTIO n -> y\n'));
const kernelConfig = kernelConfigBytes.toString('utf8');
const requiredConfig = ['CONFIG_SYSVIPC=y','CONFIG_SYSVIPC_SYSCTL=y','CONFIG_IPC_NS=y','CONFIG_SMP=y',
  'CONFIG_X86_LOCAL_APIC=y','CONFIG_X86_IO_APIC=y','CONFIG_HPET_TIMER=y',
  'CONFIG_ACPI=y','CONFIG_X86_PM_TIMER=y','CONFIG_HZ_PERIODIC=y','CONFIG_HZ_100=y','CONFIG_HZ=100',
  'CONFIG_HW_RANDOM=y','CONFIG_HW_RANDOM_VIRTIO=y','CONFIG_VIRTIO=y','CONFIG_VIRTIO_PCI=y',
  'CONFIG_X86_MPPARSE=y','# CONFIG_PCI_MSI is not set',
  '# CONFIG_HIGH_RES_TIMERS is not set','# CONFIG_NO_HZ_IDLE is not set','# CONFIG_NO_HZ_FULL is not set'];
for (const line of requiredConfig) {
  assert.equal(kernelConfig.split('\n').filter(value=>value===line).length,1,'Required actual kernel config differs: '+line);
}
const hashNames = ['bzImage','linux.config','ipc-baseline.config','acpi.config.diff',
  'acpi-baseline.config','rng.config.diff'];
const hashLines = (await fs.readFile(path.join(kernelDirectory,'build.sha256'),'utf8')).trimEnd().split('\n');
const recordedKernelHashes = {};
for (const line of hashLines) {
  const match = /^([0-9a-f]{64})  (\/out\/[^\s]+)$/.exec(line);
  assert(match,'Malformed kernel hash manifest');
  assert.equal(recordedKernelHashes[match[2]],undefined,'Duplicate kernel hash record');
  recordedKernelHashes[match[2]] = match[1];
}
assert.deepEqual(Object.keys(recordedKernelHashes).sort(),hashNames.map(name=>'/out/'+name).sort());
for (const name of hashNames) assert.equal(recordedKernelHashes['/out/'+name],kernelFiles[name].sha256);
const wasmTypeNames = ['rng-random','virtio-rng-pci'];
{
  const wasm = await fs.readFile(path.join(source,'artifacts/browser-emulator/qemu-system-x86_64.wasm'));
  for (const name of wasmTypeNames) assert(wasm.includes(Buffer.from(name+'\0')),
    'Pinned emulator lacks the expected RNG type name: '+name);
}
// String presence supports the build configuration; it does not prove realization,
// IRQ delivery, entropy credit or PostgreSQL progress. Those require the real VM.
const kernelVerification = {
  run:kernelArchive.run,head:kernelArchive.head,artifact:kernelArchive.artifact,
  archive:kernelArchive.archive,sourceCommit:kernelSourceCommit,recipe:recipeRecord,files:kernelFiles,
  baselineConfig:kernelFiles['acpi-baseline.config'],exactlyOneConfigChange:true,
  exactChange:'HW_RANDOM_VIRTIO n -> y',requiredConfig,
  embeddedEmulatorTypeNames:wasmTypeNames,entropyDeliveryVerified:false
};
configurationExperiment.kernelConfigOneFlagVerified = true;
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

// Compute the expected package from independently verified source ranges and
// the exact kernel bytes before invoking the production materializer.
assert.equal(originalMetadata.files.filter(file=>file.filename==='/pack/bzImage').length,1);
assert.equal(originalMetadata.files.filter(file=>file.filename==='/pack/vm.state').length,1);
let expectedPosition = 0;
const expectedPackageHash = createHash('sha256');
const expectedSegments = [];
for (const segment of originalMetadata.files) {
  if (segment.filename==='/pack/vm.state') continue;
  const firstByte = expectedPosition;
  const input = segment.filename==='/pack/bzImage'
    ? createReadStream(path.join(kernelDirectory,'bzImage'))
    : createReadStream(path.join(originalDirectory,'qemu-system-x86_64.data'),
        {start:segment.start,end:segment.end-1});
  for await (const bytes of input) { expectedPackageHash.update(bytes); expectedPosition+=bytes.length; }
  assert.equal(expectedPosition-firstByte,segment.filename==='/pack/bzImage'
    ? kernelFiles.bzImage.bytes : segment.end-segment.start);
  expectedSegments.push({...segment,start:firstByte,end:expectedPosition});
}
const expectedData = {bytes:expectedPosition,sha256:expectedPackageHash.digest('hex')};
const expectedMetadata = {...originalMetadata,files:expectedSegments,remote_package_size:expectedPosition};
const originalLoader = await fs.readFile(path.join(originalDirectory,'load.js'),'utf8');
const sourceTables = [...originalLoader.matchAll(/loadPackage\((\{"files":.*\})\);/g)];
assert.equal(sourceTables.length,1);
assert.deepEqual(JSON.parse(sourceTables[0][1]),originalMetadata);
const expectedLoaderText = originalLoader.replace(sourceTables[0][0],
  ()=> 'loadPackage('+JSON.stringify(expectedMetadata)+');');
const expectedLoader = bytesRecord(Buffer.from(expectedLoaderText));

// The production materializer requires its model-explorer HTML input. Copy
// the reviewed source HTML; this experiment does not build or test that demo.
await fs.mkdir(path.join(source,'browser-output'),{recursive:true});
await fs.copyFile(path.join(source,'browser-demo/index.html'),path.join(source,'browser-output/index.html'));
command('bash',['browser-single/prepare-pages.sh'],{
  env:{...process.env,BROWSER_RUNTIME_ARCHIVE:path.join(prepared,'runtime-source.zip')}
});
command('git',['diff','--exit-code']);

const derived = await readJSON(path.join(runtime,'derived-runtime.json'));
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
assert.deepEqual(derivedMetadata,expectedMetadata,'Derived segment table differs from the verified source ranges');
assert.deepEqual(await fs.readFile(path.join(runtime,'load.js')),Buffer.from(expectedLoaderText),
  'Loader changed outside the reviewed segment-table replacement');
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
assert.deepEqual(await fileRecord(path.join(source,'browser-output/wasm/launcher.js')),configurationExperiment.launcher);
assert.deepEqual(await fileRecord(path.join(source,'browser-output/wasm/log-stream.mjs')),
  {bytes:changedFiles['browser-single/pages/log-stream.mjs'].bytes,
   sha256:changedFiles['browser-single/pages/log-stream.mjs'].sha256});
assert.deepEqual(startup.emulator_build,emulator);
assert.deepEqual(Object.keys(startup.files).sort(),
  ['guest.sh','entrypoint.sh','runtime/openfga','runtime/bootstrap'].sort());
assert.equal(startup.bootstrap_build.source_commit,'207d15fab685d3adb9c5d51a048982c2236ae75c');
assert.equal(startup.bootstrap_build.native_tests,'passed');
assert.equal(startup.bootstrap_build.browser_model_timeout_seconds,30);
assert.equal(derived.startup_overlay.sha256,(await fileRecord(path.join(runtime,'startup-manifest.json'))).sha256);
const sourceFiles = {};
for (const name of ['browser-single/prepare-pages.sh','browser-single/check-pages.mjs',
  'browser-single/pages/launcher.js','browser-single/pages/log-stream.mjs','browser-single/log-stream.test.mjs',
  'browser-single/pages/startup-payload.mjs',
  'browser-single/pages/guest.sh','browser-single/entrypoint.sh',
  'browser-single/Kernel.Dockerfile','browser-single/prepare-kernel.sh','browser-single/repack-runtime.mjs',
  'e2e/package-lock.json','fga/model.json']) {
  sourceFiles[name] = await fileRecord(path.join(source,name));
}
const proof = {
  schemaVersion:1,experiment:'openfga9_schema_reachability_with_rng',diagnosticOnly:true,
  modelExplorerBuildVerified:false,sourceCommit,nativeProducerCommit,bootstrapCoreCommit,
  sourceFiles,fixtureFiles,configurationExperiment,archives,nativeVerification,lightweightVerification,
  retainedRuntimeSource:'379e3280c2a1c789071292af141648eb2fc3ffbc',
  originalRuntimeSource:originalManifest.source_commit,
  kernelSource:kernelSourceCommit,kernelVerification,
  rootfsUnchanged:true,preservedSegments,data:expectedData,loader:expectedLoader,
  args:originalManifest.files['arg-module.js'],kernel:kernelFiles.bzImage,
  kernelConfig:kernelFiles['linux.config'],requiredConfig,emulator:pair,
  startupManifest:await fileRecord(path.join(runtime,'startup-manifest.json')),
  openfgaBuild:openfga,openfgaBinary,bootstrapBuild:startup.bootstrap_build,
  executions:{initialBootstrap:'/tmp/browser-lab-runtime/bootstrap',
    probe:'/usr/local/bin/lab-tool',acceptance:'/usr/local/bin/lab-tool'},
  expectedRuntimeConfiguration:{virtualCPUs:2,memoryMiB:512,accel:'tcg,tb-size=500,thread=multi',
    kernelIRQ:'default_acpi',entropyConfiguration},
  authorizationDeadlinesChanged:false
};
await fs.writeFile(path.join(evidence,'materialization.json'),JSON.stringify(proof,null,2)+'\n');
console.log('OPENFGA9_PATHS_BROWSER_GATE '+JSON.stringify({phase:'materialized',sourceCommit,nativeProducerCommit,configurationExperiment,
  rootfsUnchanged:true,data:expectedData,loader:expectedLoader,kernel:kernelFiles.bzImage,
  nativeVerification,lightweightVerification,kernelVerification,openfgaBinary,openfgaModelEncoder:openfga.model_json_encoder,
  resolverTypesystemReuse:openfga.first_resolver_typesystem_reuse,
  bootstrapSource:startup.bootstrap_build.source_commit,bootstrapSha256:startup.bootstrap_build.binary_sha256,
  modelExplorerBuildVerified:false}));
