// Diagnostic materialization only: no builds and no execution of artifact scripts.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createReadStream, promises as fs} from 'node:fs';
import {spawnSync} from 'node:child_process';
import path from 'node:path';

const output = path.resolve('artifacts/browser-clocks-one-vcpu');
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
  {label:'current',head:'379e3280c2a1c789071292af141648eb2fc3ffbc',
    wasm:{bytes:41246941,sha256:'e85450e88a390c3494c9ce4d8fb599c8ed14afd8ae40b48deeee1bc6b3f34c0e'}}
];
assert.equal((await fileRecord('prepared/kernel/bzImage')).sha256,common.kernel);
const kernelConfig = await fs.readFile('prepared/kernel/linux.config','utf8');
for (const flag of ['CONFIG_SYSVIPC=y','CONFIG_IPC_NS=y','CONFIG_HZ_100=y','CONFIG_HZ_PERIODIC=y']) {
  assert(kernelConfig.split('\n').includes(flag),'Missing retained kernel setting: '+flag);
}

const guestPatches = [
  [
    "\"$online_cpus\" != '0-1'",
    "\"$online_cpus\" != '0'"
  ],
  [
    "expected online CPUs 0-1, got:",
    "expected diagnostic online CPU 0, got:"
  ],
  [
    "\"$allowed_cpus\" != 0-1",
    "\"$allowed_cpus\" != 0"
  ],
  [
    "expected service CPU availability 0-1, got:",
    "expected diagnostic service CPU availability 0, got:"
  ],
  [
    "# Preserve ordinary scheduling across both CPUs. Pinning the service tree to\n# CPU 0 did not prevent Firefox's observed module-compilation OOM.",
    "# Diagnostic-only: Linux must expose the one requested CPU; no affinity override."
  ],
  [
    "# The archived rootfs keeps PostgreSQL, Axum, lab-tool and the model unchanged.",
    "# Diagnostic-only clock snapshots; no timer or authorization settings change.\ndiag_clock_snapshot() {\n    local clock_id=$1 clock_point=$2 clock_uptime=unavailable clock_idle=unavailable\n    local clock_source=unavailable clock_device clock_file clock_line clock_rows=0 clock_events=0\n    printf '\\n[browser-clock] begin id=%s point=%s\\n' \"$clock_id\" \"$clock_point\"\n    read -r clock_uptime clock_idle 2>/dev/null </proc/uptime || true\n    IFS= read -r clock_source 2>/dev/null \\\n        </sys/devices/system/clocksource/clocksource0/current_clocksource || true\n    printf '[browser-clock] time id=%s uptime=%s idle=%s epoch=%s seconds=%s\\n' \\\n        \"$clock_id\" \"$clock_uptime\" \"$clock_idle\" \"${EPOCHREALTIME:-unavailable}\" \"$SECONDS\"\n    printf '[browser-clock] source id=%s value=%s\\n' \"$clock_id\" \"${clock_source:0:120}\"\n    for clock_file in /sys/devices/system/clockevents/clockevent*/current_device \\\n                      /sys/devices/system/clockevents/broadcast/current_device; do\n        [[ -r \"$clock_file\" ]] || continue\n        clock_device=unavailable\n        IFS= read -r clock_device 2>/dev/null <\"$clock_file\" || true\n        printf '[browser-clock] event id=%s path=%s value=%s\\n' \\\n            \"$clock_id\" \"$clock_file\" \"${clock_device:0:120}\"\n        clock_events=$((clock_events + 1))\n        (( clock_events < 4 )) || break\n    done\n    while IFS= read -r clock_line; do\n        case \"$clock_line\" in\n            *CPU0*|*timer*|*LOC:*|*RES:*|*CAL:*|*TLB:*|*ERR:*|*MIS:*)\n                printf '[browser-clock] irq id=%s text=%s\\n' \"$clock_id\" \"${clock_line:0:240}\"\n                clock_rows=$((clock_rows + 1))\n                (( clock_rows < 12 )) || break ;;\n        esac\n    done 2>/dev/null </proc/interrupts\n    printf '[browser-clock] end id=%s irq_rows=%s event_rows=%s\\n' \\\n        \"$clock_id\" \"$clock_rows\" \"$clock_events\"\n}\ndiag_clock_snapshot 0 before-services\n\n# The archived rootfs keeps PostgreSQL, Axum, lab-tool and the model unchanged."
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
const launcherPatches = [
  [
    "function line(value) {",
    "// Diagnostic-only: observe clean PTY lines without changing acceptance state.\nconst diagnostic = window.__browserClockDiagnostic = {\n  clockRows: [], serviceHead: [], serviceTail: [], serviceCount: 0,\n  clockError: null, serviceSummaryTruncated: false\n};\nfunction observeDiagnosticLine(value) {\n  try {\n    const row = {endSequence: window.labLogSequence, pageMonotonicMs: performance.now(), text: value};\n    if (value.startsWith('[browser-clock] ')) {\n      if (value.length > 512 || diagnostic.clockRows.length >= 3000) {\n        diagnostic.clockError = 'Clock observer exceeded its bounded row size/count';\n        return;\n      }\n      diagnostic.clockRows.push(row);\n    } else if (/^(?:LAB\\||\\[browser-(?:single|guest|openfga)\\]|waiting for server)/.test(value)) {\n      diagnostic.serviceCount++;\n      if (value.length > 512) diagnostic.serviceSummaryTruncated = true;\n      const bounded = {...row, text: value.slice(0,512), truncated: value.length > 512};\n      if (diagnostic.serviceHead.length < 8) diagnostic.serviceHead.push(bounded);\n      diagnostic.serviceTail.push(bounded);\n      if (diagnostic.serviceTail.length > 32) diagnostic.serviceTail.shift();\n    }\n  } catch (error) { diagnostic.clockError = String(error); }\n}\n\nfunction line(value) {\n  observeDiagnosticLine(value);"
  ],
  [
    "    const args = module.arguments;\n",
    "    const args = module.arguments;\n    if (!Array.isArray(args) || args.filter(value=>value==='-accel').length!==1 ||\n        args[args.indexOf('-accel')+1]!=='tcg,tb-size=500,thread=multi' ||\n        args.some(value=>['-machine','-M','-cpu','-icount','-rtc'].includes(value))) {\n      throw new Error('Diagnostic requires the exact retained MTTCG arguments and default machine');\n    }\n"
  ],
  [
    "args[smp + 1] = '2,sockets=2,cores=1,threads=1';",
    "args[smp + 1] = '1,sockets=1,cores=1,threads=1';"
  ],
  [
    "{cpus:2, reportedLogicalCPUs, cpuTopology:'2,sockets=2,cores=1,threads=1', memoryMiB:512",
    "{cpus:1, reportedLogicalCPUs, cpuTopology:'1,sockets=1,cores=1,threads=1', memoryMiB:512"
  ],
  [
    "    args[network + 1] = 'hubport,id=vmnic,hubid=0';\n",
    "    args[network + 1] = 'hubport,id=vmnic,hubid=0';\n    log('[browser-clock-host] arguments='+JSON.stringify(args)+'\\n');\n"
  ],
  [
    "    // Reject insufficient capacity before loading or requesting two vCPUs.",
    "    // Keep the production host-capacity preflight for this one-vCPU diagnostic."
  ],
  [
    "    // guest.sh independently requires both CPUs online before starting services.",
    "    // The diagnostic guest independently requires only CPU 0 online before services."
  ]
];
function replaceOnce(source,before,after) {
  assert.equal(source.split(before).length,2,'Diagnostic overlay preimage changed');
  return source.replace(before,()=>after);
}
async function applyDiagnosticOverlay(wasm,startup,recordDir) {
  const guestPath=path.join(wasm,'guest.sh'), launcherPath=path.join(wasm,'launcher.js');
  const before={guest:await fileRecord(guestPath),launcher:await fileRecord(launcherPath),
    startup:await fileRecord(path.join(wasm,'runtime/startup-manifest.json'))};
  let guest=await fs.readFile(guestPath,'utf8'),launcher=await fs.readFile(launcherPath,'utf8');
  for(const [oldText,newText] of guestPatches) guest=replaceOnce(guest,oldText,newText);
  for(const [oldText,newText] of launcherPatches) launcher=replaceOnce(launcher,oldText,newText);
  await fs.writeFile(guestPath,guest);
  await fs.writeFile(launcherPath,launcher);
  for(const [command,args,input] of [
    [process.execPath,['--input-type=module','--check'],launcher],
    [process.execPath,['--check','diagnostics/check-one-vcpu-clocks.mjs']],
    ['bash',['-n',guestPath]],
    ['bash',['-n',path.join(wasm,'entrypoint.sh')]]
  ]) {
    const checked=spawnSync(command,args,{input,encoding:'utf8',timeout:10000,maxBuffer:1024*1024});
    assert.equal(checked.status,0,'Generated diagnostic syntax failed: '+String(checked.stderr));
  }
  const manifest=structuredClone(startup);
  manifest.files['guest.sh']=await fileRecord(guestPath);
  manifest.diagnostic={kind:'one_vcpu_clock_observation',productionAcceptance:false,
    original_startup_sha256:before.startup.sha256,requested_vcpus:1,
    startup_periodic_samples:115,verification_boundary_samples_max:4,initial_samples:1,
    preserved_accel:'tcg,tb-size=500,thread=multi',host_preflight_min_logical_cpus:2};
  const manifestPath=path.join(wasm,'runtime/startup-manifest.json');
  await fs.writeFile(manifestPath,JSON.stringify(manifest,null,2)+'\n');
  assert.deepEqual(await fileRecord(path.join(wasm,'entrypoint.sh')),startup.files['entrypoint.sh']);
  assert.deepEqual(await fileRecord(path.join(wasm,'runtime/openfga')),startup.files['runtime/openfga']);
  const proof={diagnosticOnly:true,productionAcceptance:false,before,
    after:{guest:await fileRecord(guestPath),launcher:await fileRecord(launcherPath),startup:await fileRecord(manifestPath)},
    unchanged:{entrypoint:startup.files['entrypoint.sh'],openfga:startup.files['runtime/openfga']},
    patches:{guest:guestPatches,launcher:launcherPatches},
    limits:{hostMsPerEngine:1200000,guestStartupSeconds:900,requestDeadlinesSeconds:[2,3,5],clockSnapshotsMax:120}};
  await fs.writeFile(path.join(recordDir,'diagnostic-overlay.json'),JSON.stringify(proof,null,2)+'\n');
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
  assert.equal(emulator.max_instance_alive_after,15000);
  assert.equal(emulator.instance_reservation,'atomic_compare_exchange');
  assert.equal(emulator.memory_barrier_fix,'emit_atomic_fence_for_tcg_mb');
  assert.equal(emulator.barrier_emitter_postimage_blob,'4b3e9f8fabcaa3b9a2bb33746f7c1d7046691355');
  assert.equal(emulator.memory_barrier_tests,'passed');
  assert.equal(emulator.instantiate_num,1500);
  assert.equal(openfga.warmup_model_sha256,(await fileRecord(path.join(source,'fga/model.json'))).sha256);
  assert.deepEqual(emulator.files,{'out.js':common.glue,'qemu-system-x86_64.wasm':spec.wasm});
  for (const [name,expected] of Object.entries(emulator.files)) {
    assert.deepEqual(await fileRecord(path.join(directory,'emulator',name)),expected,'Wrong emulator pair member: '+name);
  }
  // Restore and verify production host/payload bytes before the diagnostic overlay.
  await fs.cp(path.resolve('source-current/browser-single/pages'),wasm,{recursive:true});
  await fs.copyFile(path.resolve('source-current/browser-single/entrypoint.sh'),path.join(wasm,'entrypoint.sh'));
  await fs.copyFile(path.join(directory,'openfga/openfga'),path.join(runtime,'openfga'));
  assert.deepEqual(Object.keys(startup.files).sort(),['entrypoint.sh','guest.sh','runtime/openfga']);
  for (const [name,expected] of Object.entries(startup.files)) {
    assert.deepEqual(await fileRecord(path.join(wasm,name)),expected,'Startup overlay changed: '+name);
  }
  assert.equal(openfga.binary_sha256,startup.files['runtime/openfga'].sha256);
  assert.deepEqual(startup.files['entrypoint.sh'],{bytes:5019,sha256:'7763b800c5f4bed8df12c6498b13474ef6adafdc4442694b7271f0b16fe3f5ed'});
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
  await applyDiagnosticOverlay(wasm,startup,recordDir);
  const diagnosticDerived=structuredClone(retained);
  diagnosticDerived.startup_overlay.sha256=(await fileRecord(path.join(runtime,'startup-manifest.json'))).sha256;
  diagnosticDerived.diagnostic={kind:'one_vcpu_clock_observation',productionAcceptance:false,requested_vcpus:1};
  diagnosticDerived.changes.push('DIAGNOSTIC ONLY: one vCPU, honest CPU guards, bounded clean-PTY clock observations');
  await fs.writeFile(path.join(runtime,'derived-runtime.json'),JSON.stringify(diagnosticDerived,null,2)+'\n');
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
const result = {diagnosticOnly:true,productionAcceptance:false,scope:'retained runtime one-vCPU clock/protocol observation',
  bundleMode:'verified_379e_runtime_diagnostic_launcher_and_guest',hostSourceHead:'379e3280c2a1c789071292af141648eb2fc3ffbc',
  buildsPerformed:false,common,prepared,sourceLogModule:await fileRecord('source-current/browser-single/pages/log-stream.mjs'),
  runner:await fileRecord('diagnostics/check-one-vcpu-clocks.mjs'),
  materializer:await fileRecord('diagnostics/prepare-one-vcpu-clocks.mjs')};
await fs.writeFile(path.join(output,'prepared.json'),JSON.stringify(result,null,2)+'\n');
console.log('DIAGNOSTIC_ONLY: retained runtime and diagnostic overlay verified; no VM started.');
