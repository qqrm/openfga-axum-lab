// Review-only until acpiKernelPin is filled from the completed reviewed kernel build.
// Never infer or accept new kernel identities from environment variables.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream,promises as fs} from 'node:fs';
import {spawnSync} from 'node:child_process';
import path from 'node:path';

const retainedHead='379e3280c2a1c789071292af141648eb2fc3ffbc';
const candidateHead='af9c5555e312ffaec3785154f5be83be7469c95f';
// Required later: {run,head,artifact,artifact_name,source_commit,archive:{bytes,sha256},
// files:{'bzImage':{bytes,sha256},'linux.config':...,'ipc-baseline.config':...,
// 'acpi.config.diff':...,'build.sha256':...,'SOURCE.txt':...}}.
// head is the artifact workflow's exact commit, source_commit is candidateHead.
const acpiKernelPin=null;
assert(acpiKernelPin,'Missing independently reviewed ACPI kernel artifact and member pins; no VM is permitted');
assert.equal(acpiKernelPin.source_commit,candidateHead);
for(const key of ['run','artifact']) assert(Number.isSafeInteger(acpiKernelPin[key])&&acpiKernelPin[key]>0);
assert(/^[a-f0-9]{40}$/.test(acpiKernelPin.head));
assert(Number.isSafeInteger(acpiKernelPin.archive.bytes)&&acpiKernelPin.archive.bytes>0);
assert(/^[a-f0-9]{64}$/.test(acpiKernelPin.archive.sha256));
const output=path.resolve('artifacts/browser-clocks-acpi');
await fs.mkdir(output,{recursive:true});
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
async function fileRecord(file,range) {
  const hash=createHash('sha256');let bytes=0;
  for await(const chunk of createReadStream(file,range)) {bytes+=chunk.length;hash.update(chunk);}
  return {bytes,sha256:hash.digest('hex')};
}
const json=async file=>JSON.parse(await fs.readFile(file,'utf8'));
const write=(file,value)=>fs.writeFile(file,JSON.stringify(value,null,2)+'\n');
const common={
  source:'99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58',
  linuxSource:'830b3c68c1fb1e9176028d02ef86f3cf76aa2476',
  retainedData:{bytes:550511872,sha256:'5735f9936e760f467571a4ee053ef56c0e3404709586a85e37d6d25c57248ef4'},
  retainedLoader:'ea9d077f640bf9e2a368193b161f48a9d268e4c4eae4ac9d8a78d318474c73d9',
  retainedKernel:'ac8d4da19cd4bcafe99bb9122ac1b6598920661fac3ad72f5fe53b62400bbd19',
  args:{bytes:739,sha256:'848ee06345b05f6a335536b6c514c510e6eddca1a0ba5121b36f8613d6de5ec8'},
  glue:{bytes:279363,sha256:'95f7e6b59d98cd7267e705aa0b2cd670970b984d13c7e81b4d4f677cd65534d2'},
  wasm:{bytes:41246941,sha256:'e85450e88a390c3494c9ce4d8fb599c8ed14afd8ae40b48deeee1bc6b3f34c0e'}
};
for(const [directory,head] of [['source-current',retainedHead],['source-candidate',candidateHead]]) {
  const checked=spawnSync('git',['-C',directory,'rev-parse','HEAD'],{encoding:'utf8',timeout:10000});
  assert.equal(checked.status,0);assert.equal(checked.stdout.trim(),head,'Source checkout changed');
  const clean=spawnSync('git',['-C',directory,'diff','--exit-code','HEAD','--'],{encoding:'utf8',timeout:10000});
  assert.equal(clean.status,0,'Tracked source checkout is dirty');
}
const kernelDir=path.resolve('prepared/acpi-kernel'),oldKernelDir=path.resolve('prepared/kernel');
const kernelArchive=await json(path.join(kernelDir,'archive-provenance.json'));
for(const key of ['run','head','artifact','artifact_name','source_commit','archive']) {
  assert.deepEqual(kernelArchive[key],acpiKernelPin[key],'New kernel archive provenance differs: '+key);
}
const kernelNames=['bzImage','linux.config','ipc-baseline.config','acpi.config.diff','build.sha256','SOURCE.txt'];
assert.deepEqual(Object.keys(acpiKernelPin.files).sort(),[...kernelNames].sort());
for(const name of kernelNames) {
  assert.deepEqual(await fileRecord(path.join(kernelDir,name)),acpiKernelPin.files[name],'New kernel member changed: '+name);
  assert.deepEqual(kernelArchive.members[name],acpiKernelPin.files[name],'Extraction provenance differs: '+name);
}
assert(acpiKernelPin.files.bzImage.bytes>1024*1024&&acpiKernelPin.files.bzImage.bytes<32*1024*1024);
assert.equal((await fileRecord(path.join(oldKernelDir,'bzImage'))).sha256,common.retainedKernel);
assert.deepEqual(await fileRecord(path.join(kernelDir,'ipc-baseline.config')),
  await fileRecord(path.join(oldKernelDir,'linux.config')),'Kernel IPC baseline is not the retained configuration');
const kernelConfig=await fs.readFile(path.join(kernelDir,'linux.config'),'utf8');
for(const flag of [
  'CONFIG_SYSVIPC=y','CONFIG_SYSVIPC_SYSCTL=y','CONFIG_IPC_NS=y','CONFIG_ACPI=y',
  'CONFIG_X86_PM_TIMER=y','CONFIG_HPET_TIMER=y','CONFIG_X86_64=y','CONFIG_SMP=y',
  'CONFIG_X86_MPPARSE=y','CONFIG_X86_LOCAL_APIC=y','CONFIG_X86_IO_APIC=y',
  'CONFIG_HZ_PERIODIC=y','CONFIG_HZ_100=y','CONFIG_HZ=100',
  '# CONFIG_HIGH_RES_TIMERS is not set','# CONFIG_NO_HZ_IDLE is not set','# CONFIG_NO_HZ_FULL is not set'
]) assert(kernelConfig.split('\n').includes(flag),'Missing reviewed kernel setting: '+flag);
const buildHashes=(await fs.readFile(path.join(kernelDir,'build.sha256'),'utf8')).trim().split('\n');
assert.equal(buildHashes.length,4);
const checkedBuildMembers=new Set();
for(const line of buildHashes) {
  const match=/^([a-f0-9]{64})  \/out\/(bzImage|linux\.config|ipc-baseline\.config|acpi\.config\.diff)$/.exec(line);
  assert(match&&!checkedBuildMembers.has(match[2]),'Unexpected kernel build hash row');
  checkedBuildMembers.add(match[2]);assert.equal(match[1],acpiKernelPin.files[match[2]].sha256);
}

async function listFiles(directory,relative='') {
  const result=[];
  for(const entry of await fs.readdir(path.join(directory,relative),{withFileTypes:true})) {
    const name=path.join(relative,entry.name);
    if(entry.isDirectory()) result.push(...await listFiles(directory,name));
    else {assert(entry.isFile(),'Unexpected source host file kind');result.push(name);}
  }
  return result.sort();
}
const retainedPages=path.resolve('source-current/browser-single/pages');
const candidatePages=path.resolve('source-candidate/browser-single/pages');
const pageNames=await listFiles(retainedPages);
assert.deepEqual(await listFiles(candidatePages),pageNames);
for(const name of pageNames.filter(name=>name!=='launcher.js')) {
  assert.deepEqual(await fileRecord(path.join(candidatePages,name)),await fileRecord(path.join(retainedPages,name)),
    'Unexpected non-launcher host change: '+name);
}
for(const name of ['browser-single/entrypoint.sh','fga/model.json']) {
  assert.deepEqual(await fileRecord(path.join('source-candidate',name)),await fileRecord(path.join('source-current',name)));
}
async function packageRecord(directory) {
  const loader=await fs.readFile(path.join(directory,'load.js'),'utf8');
  const matches=[...loader.matchAll(/loadPackage\((\{"files":.*\})\);/g)];
  assert.equal(matches.length,1,'Unknown pinned loader metadata layout');
  const metadata=JSON.parse(matches[0][1]),dataPath=path.join(directory,'qemu-system-x86_64.data');
  assert.equal(metadata.remote_package_size,(await fs.stat(dataPath)).size);
  const segments={};let position=0;
  for(const file of metadata.files) {
    assert.equal(typeof file.filename,'string');assert(!Object.hasOwn(segments,file.filename));
    assert.equal(file.start,position);assert(Number.isSafeInteger(file.end)&&file.end>file.start);
    const actual=await fileRecord(dataPath,{start:file.start,end:file.end-1});
    assert.equal(actual.bytes,file.end-file.start);segments[file.filename]=actual;position=file.end;
  }
  assert.equal(position,metadata.remote_package_size);assert(segments['/pack/rootfs.bin']);
  return {metadata,segments,data:await fileRecord(dataPath),loader:await fileRecord(path.join(directory,'load.js')),
    loaderOutsideMetadata:loader.replace(matches[0][0],'<verified-package-metadata>')};
}
function verifyRepack(original,derived,kernelRecord) {
  const expectedNames=original.metadata.files.filter(file=>file.filename!=='/pack/vm.state').map(file=>file.filename);
  assert.deepEqual(derived.metadata.files.map(file=>file.filename),expectedNames,'Repack changed segment order/set');
  assert.equal(derived.loaderOutsideMetadata,original.loaderOutsideMetadata,'Loader code outside metadata changed');
  const {files:_oldFiles,remote_package_size:_oldSize,...oldOther}=original.metadata;
  const {files:_newFiles,remote_package_size:_newSize,...newOther}=derived.metadata;
  assert.deepEqual(newOther,oldOther,'Unexpected loader metadata field change');
  for(const name of expectedNames) {
    assert.deepEqual(derived.segments[name],name==='/pack/bzImage'?kernelRecord:original.segments[name],
      'Repack changed an unexpected segment: '+name);
  }
  assert.deepEqual(derived.segments['/pack/rootfs.bin'],original.segments['/pack/rootfs.bin'],
    'Original application rootfs bytes changed');
}
function replaceOnce(source,before,after) {
  assert.equal(source.split(before).length,2,'Diagnostic overlay preimage changed');
  return source.replace(before,()=>after);
}
const guestPatches=[
  [
    "# The archived rootfs keeps PostgreSQL, Axum, lab-tool and the model unchanged.",
    "# Diagnostic-only observations; no timer, mount, scheduling or request setting changes.\ndiag_clock_snapshot() {\n    local clock_id=$1 clock_point=$2 clock_uptime=unavailable clock_idle=unavailable\n    local clock_source=unavailable clock_available=unavailable clock_device clock_file clock_line\n    local clock_events=0 clock_intr=unavailable clock_key clock_value clock_rest\n    local clock_timer=unavailable clock_sched=unavailable clock_rcu=unavailable\n    printf '\\n[browser-clock] begin id=%s point=%s\\n' \"$clock_id\" \"$clock_point\"\n    read -r clock_uptime clock_idle 2>/dev/null </proc/uptime || true\n    IFS= read -r clock_source 2>/dev/null \\\n        </sys/devices/system/clocksource/clocksource0/current_clocksource || true\n    IFS= read -r clock_available 2>/dev/null \\\n        </sys/devices/system/clocksource/clocksource0/available_clocksource || true\n    printf '[browser-clock] time id=%s uptime=%s idle=%s epoch=%s seconds=%s\\n' \\\n        \"$clock_id\" \"$clock_uptime\" \"$clock_idle\" \"${EPOCHREALTIME:-unavailable}\" \"$SECONDS\"\n    printf '[browser-clock] source id=%s value=%s\\n' \"$clock_id\" \"${clock_source:0:120}\"\n    printf '[browser-clock] available id=%s value=%s\\n' \"$clock_id\" \"${clock_available:0:240}\"\n    for clock_file in /sys/devices/system/clockevents/clockevent*/current_device \\\n                      /sys/devices/system/clockevents/broadcast/current_device; do\n        [[ -r \"$clock_file\" ]] || continue\n        clock_device=unavailable\n        IFS= read -r clock_device 2>/dev/null <\"$clock_file\" || true\n        printf '[browser-clock] event id=%s path=%s value=%s\\n' \\\n            \"$clock_id\" \"$clock_file\" \"${clock_device:0:120}\"\n        clock_events=$((clock_events + 1))\n        (( clock_events < 4 )) || break\n    done\n    while read -r clock_key clock_value clock_rest; do\n        if [[ \"$clock_key\" == intr && \"$clock_value\" =~ ^[0-9]+$ ]]; then\n            clock_intr=$clock_value\n            break\n        fi\n    done 2>/dev/null </proc/stat\n    printf '[browser-clock] intr id=%s total=%s\\n' \"$clock_id\" \"$clock_intr\"\n    while IFS= read -r clock_line; do\n        clock_key=${clock_line%%:*}\n        clock_key=${clock_key//[[:space:]]/}\n        case \"$clock_key\" in\n            TIMER) clock_timer=$clock_line ;;\n            SCHED) clock_sched=$clock_line ;;\n            RCU) clock_rcu=$clock_line ;;\n        esac\n    done 2>/dev/null </proc/softirqs\n    printf '[browser-clock] softirq id=%s name=TIMER value=%s\\n' \"$clock_id\" \"${clock_timer:0:240}\"\n    printf '[browser-clock] softirq id=%s name=SCHED value=%s\\n' \"$clock_id\" \"${clock_sched:0:240}\"\n    printf '[browser-clock] softirq id=%s name=RCU value=%s\\n' \"$clock_id\" \"${clock_rcu:0:240}\"\n    printf '[browser-clock] irq_status id=%s value=oci_masked_proc_interrupts\\n' \"$clock_id\"\n    printf '[browser-clock] end id=%s event_rows=%s softirq_rows=3\\n' \"$clock_id\" \"$clock_events\"\n}\ndiag_clock_snapshot 0 before-services\n\n# The archived rootfs keeps PostgreSQL, Axum, lab-tool and the model unchanged."
  ],
  [
    "lab_pid=$!\n",
    "lab_pid=$!\n# Bounded startup watcher; its sleep is guest time, never the host watchdog.\n(\n    trap - EXIT INT TERM\n    for ((diag_clock_id=1; diag_clock_id<=115; diag_clock_id++)); do\n        sleep 10 || break\n        diag_clock_snapshot \"$diag_clock_id\" periodic\n    done\n) &\ndiag_clock_pid=$!\n"
  ],
  [
    "trap 'kill -TERM \"$lab_pid\" 2>/dev/null || true' EXIT",
    "trap 'kill -TERM \"$lab_pid\" 2>/dev/null || true; if [[ -n \"${diag_clock_pid:-}\" ]]; then kill -TERM \"$diag_clock_pid\" 2>/dev/null || true; fi' EXIT"
  ],
  [
    "printf 'LAB|ready\\n'\n",
    "# Stop the asynchronous writer before the original multi-write LAB result frame.\nkill -TERM \"$diag_clock_pid\" 2>/dev/null || true\nwait \"$diag_clock_pid\" 2>/dev/null || true\ndiag_clock_pid=\nprintf 'LAB|ready\\n'\n"
  ],
  [
    "    printf 'LAB|verification-start|%s\\n' \"$verification_id\"\n",
    "    if (( verification_id <= 2 )); then\n        diag_clock_snapshot \"$((1000 + verification_id * 2 - 1))\" before-verification\n    fi\n    printf 'LAB|verification-start|%s\\n' \"$verification_id\"\n"
  ],
  [
    "        cat /tmp/browser-lab-result.json\n",
    "        cat /tmp/browser-lab-result.json\n        if (( verification_id <= 2 )); then\n            diag_clock_snapshot \"$((1000 + verification_id * 2))\" after-verification\n        fi\n"
  ],
  [
    "        tail -c 4000 /tmp/browser-lab-test-error.log\n",
    "        if (( verification_id <= 2 )); then\n            diag_clock_snapshot \"$((1000 + verification_id * 2))\" verification-error\n        fi\n        tail -c 4000 /tmp/browser-lab-test-error.log\n"
  ]
];
const launcherPatches=[
  [
    "function line(value) {",
    "// Diagnostic-only: observe clean PTY lines without changing acceptance state.\nconst diagnostic = window.__browserClockDiagnostic = {\n  clockRows: [], serviceHead: [], serviceTail: [], serviceCount: 0,\n  clockError: null, serviceSummaryTruncated: false\n};\nfunction observeDiagnosticLine(value) {\n  try {\n    const row = {endSequence: window.labLogSequence, pageMonotonicMs: performance.now(), text: value};\n    if (value.startsWith('[browser-clock] ')) {\n      if (value.length > 512 || diagnostic.clockRows.length >= 3000) {\n        diagnostic.clockError = 'Clock observer exceeded its bounded row size/count';\n        return;\n      }\n      diagnostic.clockRows.push(row);\n    } else if (/^(?:LAB\\||\\[browser-(?:single|guest|openfga)\\]|waiting for server)/.test(value)) {\n      diagnostic.serviceCount++;\n      if (value.length > 512) diagnostic.serviceSummaryTruncated = true;\n      const bounded = {...row, text: value.slice(0,512), truncated: value.length > 512};\n      if (diagnostic.serviceHead.length < 8) diagnostic.serviceHead.push(bounded);\n      diagnostic.serviceTail.push(bounded);\n      if (diagnostic.serviceTail.length > 32) diagnostic.serviceTail.shift();\n    }\n  } catch (error) { diagnostic.clockError = String(error); }\n}\n\nfunction line(value) {\n  observeDiagnosticLine(value);"
  ],
  [
    "    const args = module.arguments;\n",
    "    const args = module.arguments;\n    if (!Array.isArray(args) || args.filter(value=>value==='-accel').length!==1 ||\n        args[args.indexOf('-accel')+1]!=='tcg,tb-size=500,thread=multi' ||\n        args.some(value=>['-machine','-M','-cpu','-icount','-rtc'].includes(value))) {\n      throw new Error('Diagnostic requires the exact retained MTTCG arguments and default machine');\n    }\n"
  ],
  [
    "    args[network + 1] = 'hubport,id=vmnic,hubid=0';\n",
    "    args[network + 1] = 'hubport,id=vmnic,hubid=0';\n    log('[browser-clock-host] arguments='+JSON.stringify(args)+'\\n');\n"
  ]
];

const directory=path.resolve('prepared/current'),wasm=path.join(directory,'browser-output/wasm');
const runtime=path.join(wasm,'runtime'),recordDir=path.join(output,'provenance');
await fs.mkdir(recordDir,{recursive:true});
const archived=await json(path.join(directory,'archive-provenance.json'));
assert.equal(archived.head,retainedHead);assert.equal(archived.run,43402635950);assert.equal(archived.artifact,14029511154);
assert.deepEqual(archived.archive,{bytes:260830150,sha256:'73e4c4d2f2204ef4da0450e472b50ffb507e52b6a46fe55e0cab153b4fa3976e'});
const oldKernelArchive=await json(path.join(oldKernelDir,'archive-provenance.json'));
assert.equal(oldKernelArchive.head,retainedHead);assert.equal(oldKernelArchive.run,43402635950);assert.equal(oldKernelArchive.artifact,14026387821);
assert.deepEqual(oldKernelArchive.archive,{bytes:3360331,sha256:'b26572994ab8e2b2904c4f5b88fc039a58ed54b58b74c58f6cbaf39f60568812'});
const retained=await json(path.join(directory,'retained/derived-runtime.json'));
const startupBytes=await fs.readFile(path.join(directory,'retained/startup-manifest.json'));
const startup=JSON.parse(startupBytes),emulator=await json(path.join(directory,'emulator/build.json'));
const openfga=await json(path.join(directory,'openfga/build.json'));
assert.equal(retained.source,common.source);assert.equal(retained.linux_source,common.linuxSource);assert.equal(retained.rootfs_changed,false);
assert.deepEqual(retained.data,common.retainedData);assert.equal(retained.loader_sha256,common.retainedLoader);
assert.equal(retained.kernel_sha256,common.retainedKernel);assert.deepEqual(retained.emulator_build,emulator);
assert.deepEqual(startup.emulator_build,emulator);assert.deepEqual(startup.openfga_build,openfga);
assert.equal(startup.base_source_commit,common.source);assert.equal(retained.startup_overlay.manifest,'startup-manifest.json');
assert.equal(retained.startup_overlay.sha256,sha(startupBytes));
assert.equal(emulator.qemu_commit,'8604ed49a3cde392890b014a8d5a959c8a2fe72a');
assert.equal(emulator.backend_postimage_blob,'e0b26bdc8d6d5b987061f9090ae0b62b76c34e16');
assert.equal(emulator.instance_budget_configuration,'shared_c_pre_run_setter');
assert.equal(emulator.firefox_instance_limit,8192);assert.equal(emulator.chromium_instance_limit,15000);
assert.equal(emulator.max_instance_running_local,2048);assert.equal(emulator.max_instance_alive_after,15000);
assert.equal(emulator.instance_reservation,'atomic_compare_exchange');assert.equal(emulator.instantiate_num,1500);
assert.equal(emulator.memory_barrier_fix,'emit_atomic_fence_for_tcg_mb');
assert.equal(emulator.barrier_emitter_postimage_blob,'4b3e9f8fabcaa3b9a2bb33746f7c1d7046691355');
assert.equal(emulator.memory_barrier_tests,'passed');
assert.equal(openfga.warmup_model_sha256,(await fileRecord('source-current/fga/model.json')).sha256);
assert.deepEqual(emulator.files,{'out.js':common.glue,'qemu-system-x86_64.wasm':common.wasm});
for(const [name,expected] of Object.entries(emulator.files)) assert.deepEqual(await fileRecord(path.join(directory,'emulator',name)),expected);
const originalManifest=await json(path.join(retainedPages,'runtime-manifest.json'));
assert.equal(originalManifest.source_commit,common.source);
assert.equal(originalManifest.archive_sha256,'fd7511caa98399d245adce39feb3398884f8dc56a82aa44c625e6fa46bf0acee');
assert.equal((await fileRecord(path.join(directory,'runtime-source.zip'))).sha256,originalManifest.archive_sha256);
assert.deepEqual(Object.keys(originalManifest.files).sort(),['arg-module.js','load.js','out.js','qemu-system-x86_64.data','qemu-system-x86_64.wasm']);
for(const [name,expected] of Object.entries(originalManifest.files)) assert.deepEqual(await fileRecord(path.join(runtime,name)),expected);
const original=await packageRecord(runtime);
assert(original.segments['/pack/vm.state'],'Missing original unused migration snapshot');
const candidateRuntime=path.join(directory,'candidate-runtime');
await fs.mkdir(candidateRuntime); // Exclusive fresh output, never reuse partial materialization.
for(const name of ['load.js','qemu-system-x86_64.data']) await fs.copyFile(path.join(runtime,name),path.join(candidateRuntime,name));
async function repack(source,input,kernel,label) {
  const result=spawnSync(process.execPath,[path.join(source,'browser-single/repack-runtime.mjs'),input,kernel],
    {encoding:'utf8',timeout:60000,maxBuffer:1024*1024});
  await fs.writeFile(path.join(recordDir,label+'-repack.log'),(result.stdout??'')+(result.stderr??''));
  assert.equal(result.status,0,label+' pinned kernel repack failed');
  return json(path.join(input,'derived-runtime.json'));
}
// Establish the exact old retained identity before constructing any new derived package.
const rebuiltOld=await repack(path.resolve('source-current'),runtime,oldKernelDir,'retained');
const oldPackage=await packageRecord(runtime);
assert.deepEqual(oldPackage.data,common.retainedData);assert.equal(oldPackage.loader.sha256,common.retainedLoader);
verifyRepack(original,oldPackage,await fileRecord(path.join(oldKernelDir,'bzImage')));
for(const key of ['source','linux_source','rootfs_changed','data','loader_sha256','kernel_sha256']) assert.deepEqual(rebuiltOld[key],retained[key]);
// This second input is the independently verified original package, never the old derived package.
const rebuiltNew=await repack(path.resolve('source-candidate'),candidateRuntime,kernelDir,'acpi');
const newPackage=await packageRecord(candidateRuntime);
verifyRepack(original,newPackage,acpiKernelPin.files.bzImage);
assert.equal(rebuiltNew.source,common.source);assert.equal(rebuiltNew.linux_source,common.linuxSource);
assert.equal(rebuiltNew.rootfs_changed,false);assert.deepEqual(rebuiltNew.data,newPackage.data);
assert.equal(rebuiltNew.loader_sha256,newPackage.loader.sha256);
assert.equal(rebuiltNew.kernel_sha256,acpiKernelPin.files.bzImage.sha256);
for(const name of ['qemu-system-x86_64.data','load.js']) await fs.rename(path.join(candidateRuntime,name),path.join(runtime,name));
await fs.rm(path.join(runtime,'kernel-build'),{recursive:true});
await fs.rename(path.join(candidateRuntime,'kernel-build'),path.join(runtime,'kernel-build'));
assert.deepEqual(await fileRecord(path.join(runtime,'arg-module.js')),common.args);
// Host identity is the separately pinned candidate; application payload remains the retained 379e bytes.
await fs.cp(candidatePages,wasm,{recursive:true});
await fs.copyFile('source-current/browser-single/entrypoint.sh',path.join(wasm,'entrypoint.sh'));
await fs.copyFile(path.join(directory,'openfga/openfga'),path.join(runtime,'openfga'));
assert.deepEqual(Object.keys(startup.files).sort(),['entrypoint.sh','guest.sh','runtime/openfga']);
for(const [name,expected] of Object.entries(startup.files)) assert.deepEqual(await fileRecord(path.join(wasm,name)),expected);
assert.equal(openfga.binary_sha256,startup.files['runtime/openfga'].sha256);
assert.deepEqual(startup.files['entrypoint.sh'],{bytes:5019,sha256:'7763b800c5f4bed8df12c6498b13474ef6adafdc4442694b7271f0b16fe3f5ed'});
for(const [name,expected] of Object.entries(emulator.files)) {
  await fs.copyFile(path.join(directory,'emulator',name),path.join(runtime,name));
  assert.deepEqual(await fileRecord(path.join(runtime,name)),expected);
}
const guestPath=path.join(wasm,'guest.sh'),launcherPath=path.join(wasm,'launcher.js');
const before={guest:await fileRecord(guestPath),launcher:await fileRecord(launcherPath),startup:{bytes:startupBytes.length,sha256:sha(startupBytes)}};
let guest=await fs.readFile(guestPath,'utf8'),launcher=await fs.readFile(launcherPath,'utf8');
for(const [oldText,newText] of guestPatches) guest=replaceOnce(guest,oldText,newText);
for(const [oldText,newText] of launcherPatches) launcher=replaceOnce(launcher,oldText,newText);
await fs.writeFile(guestPath,guest);await fs.writeFile(launcherPath,launcher);
for(const [command,args,input] of [
  [process.execPath,['--input-type=module','--check'],launcher],
  [process.execPath,['--check','diagnostics/check-acpi-clocks.mjs']],
  ['bash',['-n',guestPath]],['bash',['-n',path.join(wasm,'entrypoint.sh')]]
]) {
  const checked=spawnSync(command,args,{input,encoding:'utf8',timeout:10000,maxBuffer:1024*1024});
  assert.equal(checked.status,0,'Generated diagnostic syntax failed: '+String(checked.stderr));
}
const manifest=structuredClone(startup);
manifest.files['guest.sh']=await fileRecord(guestPath);
manifest.diagnostic={kind:'two_vcpu_acpi_clock_observation',productionAcceptance:false,
  original_startup_sha256:sha(startupBytes),runtime_source_commit:retainedHead,host_source_commit:candidateHead,
  requested_vcpus:2,acpi_discovery_enabled:true,kernel:{artifact:acpiKernelPin.artifact,
    archive_sha256:acpiKernelPin.archive.sha256,bzImage:acpiKernelPin.files.bzImage},
  startup_periodic_samples:115,verification_boundary_samples_max:4,initial_samples:1,
  preserved_accel:'tcg,tb-size=500,thread=multi',host_preflight_min_logical_cpus:2};
await write(path.join(runtime,'startup-manifest.json'),manifest);
const rootfsProof={original:original.segments['/pack/rootfs.bin'],retained:oldPackage.segments['/pack/rootfs.bin'],
  derived:newPackage.segments['/pack/rootfs.bin'],equal:true};
const derived={...rebuiltNew,emulator_build:emulator,
  startup_overlay:{manifest:'startup-manifest.json',sha256:(await fileRecord(path.join(runtime,'startup-manifest.json'))).sha256},
  diagnostic:{kind:'two_vcpu_acpi_clock_observation',productionAcceptance:false,requested_vcpus:2,
    runtime_source_commit:retainedHead,host_source_commit:candidateHead,kernel_artifact:acpiKernelPin.artifact},
  rootfs_proof:rootfsProof,
  changes:[...rebuiltNew.changes,'Preserve the verified retained QEMU pair and OpenFGA/entrypoint payload',
    'DIAGNOSTIC ONLY: bounded clean-PTY clocks and source interrupt counters; original two CPU guards and request budgets']};
await write(path.join(runtime,'derived-runtime.json'),derived);
const files={};
for(const name of [...pageNames,'entrypoint.sh','runtime/startup-manifest.json','runtime/derived-runtime.json',
  'runtime/openfga','runtime/out.js','runtime/qemu-system-x86_64.wasm','runtime/load.js','runtime/arg-module.js']) {
  files[name]=await fileRecord(path.join(wasm,name));
}
files['runtime/qemu-system-x86_64.data']=newPackage.data;
assert.deepEqual(files['entrypoint.sh'],startup.files['entrypoint.sh']);
assert.deepEqual(files['runtime/openfga'],startup.files['runtime/openfga']);
await write(path.join(recordDir,'rootfs-and-segments.json'),{
  rootfs:rootfsProof,original:{data:original.data,loader:original.loader,segments:original.segments},
  retained:{data:oldPackage.data,loader:oldPackage.loader,segments:oldPackage.segments},
  derived:{data:newPackage.data,loader:newPackage.loader,segments:newPackage.segments}});
await write(path.join(recordDir,'diagnostic-overlay.json'),{diagnosticOnly:true,productionAcceptance:false,before,
  after:{guest:files['guest.sh'],launcher:files['launcher.js'],startup:files['runtime/startup-manifest.json']},
  patches:{guest:guestPatches,launcher:launcherPatches},
  limits:{hostMsPerEngine:1200000,guestStartupSeconds:900,requestDeadlinesSeconds:[2,3,5],clockSnapshotsMax:120,clockRowsPerSnapshotMax:14}});
for(const name of ['archive-provenance.json','retained/startup-manifest.json','retained/derived-runtime.json','emulator/build.json','openfga/build.json']) {
  await fs.copyFile(path.join(directory,name),path.join(recordDir,name.replaceAll('/','-')));
}
await write(path.join(recordDir,'new-kernel-pin.json'),acpiKernelPin);
for(const name of ['archive-provenance.json',...kernelNames]) await fs.copyFile(path.join(kernelDir,name),path.join(recordDir,'acpi-'+name));
await fs.copyFile(path.join(oldKernelDir,'archive-provenance.json'),path.join(recordDir,'retained-kernel-archive-provenance.json'));
const result={diagnosticOnly:true,productionAcceptance:false,scope:'two-vCPU ACPI retained-runtime clock/protocol observation',
  bundleMode:'verified_379e_payload_af9c_host_new_pinned_acpi_kernel',hostSourceHead:candidateHead,runtimeSourceHead:retainedHead,
  buildsPerformed:false,common,kernelPin:acpiKernelPin,rootfsProof,
  prepared:[{label:'current',head:retainedHead,hostHead:candidateHead,root:path.dirname(wasm),files,archived}],
  sourceLogModule:await fileRecord('source-current/browser-single/pages/log-stream.mjs'),
  runner:await fileRecord('diagnostics/check-acpi-clocks.mjs'),
  materializer:await fileRecord('diagnostics/prepare-acpi-clocks.mjs')};
await write(path.join(output,'prepared.json'),result);
console.log('DIAGNOSTIC_ONLY '+JSON.stringify({prepared:true,requestedVcpus:2,acpi:true,
  runtimeSourceHead:retainedHead,hostSourceHead:candidateHead,kernelArtifact:acpiKernelPin.artifact,
  data:newPackage.data,loader:newPackage.loader,kernel:acpiKernelPin.files.bzImage,rootfs:rootfsProof}));
