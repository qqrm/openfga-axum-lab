import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';

export const baselineSource = '444b069daf16e9606a0bbf1ea0aea7f4d61b97d8';
const pinned = {
  guest: {bytes:6354,sha256:'213c1e9d0938c5082ffbc6b96086b3d3aa21ab2dc09c58afa1d2b17b1ef80649'},
  entrypoint: {bytes:5367,sha256:'930a48ef13f3df61b4bc09170f512fc7279b6c537a99eff80f24038126687225'},
};
const record = text => ({bytes:Buffer.byteLength(text),sha256:createHash('sha256').update(text).digest('hex')});

// All observations use shell builtins. SECONDS is local to its shell; uptime
// is the common guest-kernel clock. No command text, arguments or secrets log.
const entrypointObservers = String.raw`
stack_phase_name=entrypoint-setup
stack_exit_reason=entrypoint-command-failed
stack_process_observed=not-started
stack_process_state() {
    local pid=$1 key value
    stack_process_observed=not-started
    if [[ ! "$pid" =~ ^[1-9][0-9]*$ ]]; then return 0; fi
    stack_process_observed=exited
    if kill -0 "$pid" 2>/dev/null; then
        stack_process_observed=unknown
        if [[ -r "/proc/$pid/status" ]]; then
            while read -r key value _; do
                if [[ "$key" == State: && "$value" =~ ^[A-Z]$ ]]; then
                    stack_process_observed=$value
                    break
                fi
            done 2>/dev/null <"/proc/$pid/status" || true
        fi
    fi
}
stack_phase() {
    stack_phase_name=$1
    local uptime_seconds=unavailable
    read -r uptime_seconds _ </proc/uptime || true
    printf 'STACK|phase|{"phase":"%s","datastore":"%s","guest_seconds":%s,"uptime_seconds":"%s","clock_scope":"entrypoint"}\n' \
        "$stack_phase_name" "$OPENFGA_DATASTORE_ENGINE" "$SECONDS" "$uptime_seconds" || true
}
stack_entrypoint_exit() {
    local exit_code=$1 uptime_seconds=unavailable observed_app_pid=0 observed_fga_pid=0 app_state fga_state
    read -r uptime_seconds _ </proc/uptime || true
    if [[ -n "$app_pid" ]]; then observed_app_pid=$app_pid; fi
    if [[ -n "$fga_pid" ]]; then observed_fga_pid=$fga_pid; fi
    stack_process_state "$observed_app_pid"
    app_state=$stack_process_observed
    stack_process_state "$observed_fga_pid"
    fga_state=$stack_process_observed
    printf 'STACK|entrypoint-exit|{"reason":"%s","phase":"%s","datastore":"%s","exit_code":%s,"guest_seconds":%s,"uptime_seconds":"%s","clock_scope":"entrypoint","app_pid":%s,"fga_pid":%s,"app_state":"%s","fga_state":"%s"}\n' \
        "$stack_exit_reason" "$stack_phase_name" "$OPENFGA_DATASTORE_ENGINE" "$exit_code" "$SECONDS" "$uptime_seconds" \
        "$observed_app_pid" "$observed_fga_pid" "$app_state" "$fga_state" || true
}
`;

const guestObserver = String.raw`
stack_guest_failure() {
    local reason=$1 child_pid=$2 uptime_seconds=unavailable child_state=exited key value
    read -r uptime_seconds _ </proc/uptime || true
    if kill -0 "$child_pid" 2>/dev/null; then
        child_state=unknown
        if [[ -r "/proc/$child_pid/status" ]]; then
            while read -r key value _; do
                if [[ "$key" == State: && "$value" =~ ^[A-Z]$ ]]; then
                    child_state=$value
                    break
                fi
            done 2>/dev/null <"/proc/$child_pid/status" || true
        fi
    fi
    printf 'STACK|guest-failure|{"reason":"%s","guest_seconds":%s,"uptime_seconds":"%s","clock_scope":"guest-wrapper","readiness_deadline_seconds":900,"child_pid":%s,"child_state":"%s"}\n' \
        "$reason" "$SECONDS" "$uptime_seconds" "$child_pid" "$child_state" || true
}
`;

function uniqueReplace(text, before, after, label) {
  assert.notEqual(before,after,`${label}: empty edit`);
  const index=text.indexOf(before);
  assert.ok(index>=0 && text.indexOf(before,index+before.length)<0,`${label}: expected one exact anchor`);
  return text.slice(0,index)+after+text.slice(index+before.length);
}

/** Derive a reviewed full-stack guest from the exact frozen runtime sources. */
export function transformStackRuntime({guest,entrypoint,datastore}) {
  assert.ok(datastore==='postgres'||datastore==='memory','datastore must be postgres or memory');
  const originals={guest,entrypoint};
  const texts={guest,entrypoint};
  for(const name of Object.keys(pinned)) {
    assert.equal(typeof texts[name],'string',`${name}: expected UTF-8 source string`);
    assert.deepEqual(record(texts[name]),pinned[name],`${name}: source differs from pinned runtime`);
  }
  const edits=[];
  const edit=(file,label,before,after,functional=false)=>{
    texts[file]=uniqueReplace(texts[file],before,after,label);
    edits.push({file,label,before,after,functional});
  };
  const phase=(name)=>`stack_phase ${name}\n`;
  const surround=(label,command,start,end)=>edit('entrypoint',label,command,phase(start)+command+phase(end));

  edit('entrypoint','builtin phase and process observations','app_pid=\n','app_pid=\n'+entrypointObservers);
  edit('entrypoint','observe exit before cleanup',
    '    trap - EXIT INT TERM\n    for pid in "$app_pid" "$fga_pid"; do\n',
    '    trap - EXIT INT TERM\n    stack_entrypoint_exit "$status" || true\n    for pid in "$app_pid" "$fga_pid"; do\n');
  edit('entrypoint','classify interrupt',"trap 'exit 130' INT\n","trap 'stack_exit_reason=signal-int; exit 130' INT\n");
  edit('entrypoint','classify termination',"trap 'exit 143' TERM\n","trap 'stack_exit_reason=signal-term; exit 143' TERM\n");

  surround('initdb phase',String.raw`initdb -D "$PGDATA" -U "$PGUSER" --auth=trust --encoding=UTF8 --no-instructions \
    --set=max_connections=60 --set=shared_buffers=16MB \
    >"$runtime_dir/initdb.log" 2>&1 || { cat "$runtime_dir/initdb.log" >&2; exit 1; }
`,'initdb-start','initdb-complete');
  surround('PostgreSQL startup phase',String.raw`pg_ctl -D "$PGDATA" -l "$runtime_dir/postgres.log" \
    -o "-c listen_addresses=127.0.0.1 -c unix_socket_directories=$runtime_dir -c shared_buffers=16MB -c max_connections=60" \
    -w start || { cat "$runtime_dir/postgres.log" >&2; exit 1; }
postgres_started=true
`,'postgres-start','postgres-ready');
  surround('OpenFGA database creation phase','createdb -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" openfga\n',
    'createdb-openfga-start','createdb-openfga-complete');
  surround('application database creation phase','createdb -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" authzlab\n',
    'createdb-authzlab-start','createdb-authzlab-complete');
  surround('OpenFGA SQL migration phase','"$openfga_binary" migrate\n','openfga-migrate-start','openfga-migrate-complete');
  surround('OpenFGA process phase','"$openfga_binary" run &\nfga_pid=$!\n','openfga-start','openfga-started');
  edit('entrypoint','listener phase','http_listening=false\n',phase('openfga-listener-start')+'http_listening=false\n');
  edit('entrypoint','classify listener child exit',
    '        echo "[browser-single] OpenFGA exited before its HTTP listener started" >&2\n',
    '        stack_exit_reason=openfga-exited-before-listener\n        echo "[browser-single] OpenFGA exited before its HTTP listener started" >&2\n');
  edit('entrypoint','classify listener deadline',
    '    echo "[browser-single] OpenFGA HTTP listener startup timed out" >&2\n',
    '    stack_exit_reason=openfga-listener-deadline\n    echo "[browser-single] OpenFGA HTTP listener startup timed out" >&2\n');
  edit('entrypoint','listener ready phase',
    'echo "[browser-single] binding the pinned Lakekeeper model and seeding real SQL data"\n',
    phase('openfga-listener-ready')+'echo "[browser-single] binding the pinned Lakekeeper model and seeding real SQL data"\n');
  surround('bootstrap phase','"$bootstrap_binary" bootstrap\n','bootstrap-start','bootstrap-complete');
  surround('application process phase','/usr/local/bin/app &\napp_pid=$!\n','app-start','app-started');
  edit('entrypoint','observe required-service exit',
    'set +e\nwait -n "$app_pid" "$fga_pid"\nstatus=$?\nset -e\n',
    phase('app-wait')+'set +e\nwait -n "$app_pid" "$fga_pid"\nstatus=$?\nset -e\n'+
    'stack_exit_reason=required-service-exited\n'+phase('app-wait-finished'));

  edit('guest','builtin readiness failure observation','umask 077\n','umask 077\n'+guestObserver);
  edit('guest','default readiness exit reason','ready=false\nwhile (( SECONDS < deadline )); do\n',
    'ready=false\nstack_failure_reason=readiness-deadline\nwhile (( SECONDS < deadline )); do\n');
  edit('guest','classify readiness child exit','    if ! kill -0 "$lab_pid" 2>/dev/null; then break; fi\n',
    '    if ! kill -0 "$lab_pid" 2>/dev/null; then\n        stack_failure_reason=entrypoint-exited\n        break\n    fi\n');
  edit('guest','observe failure before bounded diagnostics',
    'if [[ "$ready" != true ]]; then\n    # One failure snapshot: 8 seconds plus 1 second to force termination.\n',
    'if [[ "$ready" != true ]]; then\n    stack_guest_failure "$stack_failure_reason" "$lab_pid"\n'+
    '    # One failure snapshot: 8 seconds plus 1 second to force termination.\n');

  if(datastore==='memory') {
    edit('entrypoint','select built-in memory datastore','export OPENFGA_DATASTORE_ENGINE=postgres\n',
      'export OPENFGA_DATASTORE_ENGINE=memory\n',true);
    edit('entrypoint','skip OpenFGA-only SQL migration','"$openfga_binary" migrate\n',
      phase('openfga-migrate-skipped'),true);
  }

  // Prove every declared insertion/replacement can restore the exact source.
  const inverse={...texts};
  for(const change of [...edits].reverse()) {
    inverse[change.file]=uniqueReplace(inverse[change.file],change.after,change.before,'inverse '+change.label);
  }
  assert.deepEqual(inverse,originals,'inverse transformation did not restore frozen sources');
  const acceptanceAnchor='\npostgres_pid=\n';
  assert.equal(texts.guest.slice(texts.guest.indexOf(acceptanceAnchor)),
    guest.slice(guest.indexOf(acceptanceAnchor)),'production acceptance tail changed');
  for(const command of ['createdb -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" openfga\n',
    'createdb -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" authzlab\n',
    '"$bootstrap_binary" bootstrap\n','/usr/local/bin/app &\napp_pid=$!\n']) {
    assert.equal(texts.entrypoint.split(command).length,2,'required production command changed');
  }
  return {...texts,proof:{
    kind:'verified_full_stack_runtime_transform',baselineSource,datastore,
    input:pinned,output:{guest:record(texts.guest),entrypoint:record(texts.entrypoint)},
    inverseRestoresPinnedSources:true,productionAcceptanceTailUnchanged:true,
    postgresApplicationStartupUnchanged:true,postgresBothDatabasesCreated:true,
    authorizationDeadlinesChanged:false,authorizationWorkloadChanged:false,
    additionalAuthorizationWarmup:false,originalLabToolAcceptance:true,
    observationProtocol:'STACK|phase, STACK|entrypoint-exit, STACK|guest-failure; original LAB protocol unchanged',
    observationClock:'guest_seconds is local SECONDS; clock_scope identifies shell; uptime_seconds is shared Linux uptime',
    edits:edits.map(({file,label,before,after,functional})=>({file,label,functional,before:record(before),after:record(after)})),
    functionalChanges:edits.filter(change=>change.functional).map(change=>change.label),
    executionVerified:false,
  }};
}
