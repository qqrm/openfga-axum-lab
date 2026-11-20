import assert from 'node:assert/strict';
import {execFileSync,spawnSync} from 'node:child_process';
import {existsSync} from 'node:fs';
import test from 'node:test';
import {baselineSource,transformStackRuntime} from './stack-runtime-transform.mjs';

// CI already checks out the exact source before downloading runtime artifacts.
// The shared diagnostic checkout also retains the same commit as a Git object.
const gitPrefix=existsSync('source-candidate/.git')?['-C','source-candidate']:[];
const source=path=>execFileSync('git',[...gitPrefix,'show',`${baselineSource}:${path}`],{encoding:'utf8'});
const inputs={guest:source('browser-single/pages/guest.sh'),entrypoint:source('browser-single/entrypoint.sh')};
const outputs=Object.fromEntries(['postgres','memory'].map(datastore=>[datastore,transformStackRuntime({...inputs,datastore})]));

test('both pinned transforms restore source and retain the complete acceptance tail',()=>{
  const anchor='\npostgres_pid=\n';
  for(const datastore of ['postgres','memory']) {
    const transformed=outputs[datastore];
    assert.equal(transformed.proof.inverseRestoresPinnedSources,true);
    assert.equal(transformed.guest.slice(transformed.guest.indexOf(anchor)),inputs.guest.slice(inputs.guest.indexOf(anchor)));
    for(const script of ['guest','entrypoint']) execFileSync('bash',['-n'],{input:transformed[script]});
    assert.equal(transformed.proof.authorizationDeadlinesChanged,false);
    assert.equal(transformed.proof.executionVerified,false);
  }
});

test('the only differences between arms are OpenFGA engine and its SQL migration',()=>{
  assert.equal(outputs.memory.guest,outputs.postgres.guest);
  assert.equal(outputs.memory.entrypoint
    .replace('export OPENFGA_DATASTORE_ENGINE=memory\n','export OPENFGA_DATASTORE_ENGINE=postgres\n')
    .replace('stack_phase openfga-migrate-skipped\n','"$openfga_binary" migrate\n'),outputs.postgres.entrypoint);
  assert.deepEqual(outputs.postgres.proof.functionalChanges,[]);
  assert.equal(outputs.memory.proof.functionalChanges.length,2);
});

test('modified source, repeated transformation and unknown datastore are rejected',()=>{
  assert.throws(()=>transformStackRuntime({...inputs,guest:inputs.guest+'\n',datastore:'postgres'}),/source differs/);
  assert.throws(()=>transformStackRuntime({...inputs,entrypoint:inputs.entrypoint.replace('shared_buffers=16MB','shared_buffers=32MB'),datastore:'memory'}),/source differs/);
  assert.throws(()=>transformStackRuntime({...outputs.memory,datastore:'memory'}),/source differs/);
  assert.throws(()=>transformStackRuntime({...inputs,datastore:'sqlite'}),/datastore must/);
});

const between=(text,start,end)=>{
  const first=text.indexOf(start);
  const last=text.indexOf(end,first+start.length);
  assert.ok(first>=0&&last>first);
  return text.slice(first,last);
};
const guest=outputs.postgres.guest;
const failureHelper=between(guest,'stack_guest_failure() {','\nexport BROWSER_LAB_EPHEMERAL=yes');
const readiness=between(guest,'ready=false\n','    # One failure snapshot: 8 seconds plus 1 second to force termination.');

for(const reason of ['entrypoint-exited','readiness-deadline']) {
  test(`actual readiness branch records ${reason} before diagnostic collection`,()=>{
    // No service is started: the original probe is short-circuited, and the
    // actual bounded diagnostic body is replaced by a visible boundary only.
    const setup=reason==='entrypoint-exited'
      ?'SECONDS=0\ndeadline=900\nlab_pid=99999999\n'
      :'SECONDS=900\ndeadline=900\nlab_pid=$$\n';
    const script='set -uo pipefail\n'+failureHelper+'\ngrep() { return 1; }\n'+setup+readiness+
      "    printf 'TEST_DIAGNOSTICS_REACHED\\n'\nfi\n";
    const result=spawnSync('bash',[],{input:script,encoding:'utf8',timeout:3000});
    assert.equal(result.status,0,result.stderr);
    const lines=result.stdout.trim().split('\n');
    assert.match(lines[0],/^STACK\|guest-failure\|/);
    assert.equal(lines[1],'TEST_DIAGNOSTICS_REACHED');
    const observation=JSON.parse(lines[0].slice('STACK|guest-failure|'.length));
    assert.equal(observation.reason,reason);
    assert.equal(observation.readiness_deadline_seconds,900);
    assert.equal(observation.clock_scope,'guest-wrapper');
    assert.match(observation.uptime_seconds,/^\d+(\.\d+)?$/);
    if(reason==='entrypoint-exited') assert.equal(observation.child_state,'exited');
    else assert.match(observation.child_state,/^[A-Z]$/);
  });
}

test('entrypoint EXIT observation preserves the original failing exit status',()=>{
  const entrypoint=outputs.postgres.entrypoint;
  const observers=between(entrypoint,'stack_phase_name=entrypoint-setup\n','# cleanup is registered as an EXIT/INT/TERM handler');
  const cleanup=between(entrypoint,'cleanup() {','\necho "[browser-single] initializing isolated PostgreSQL 17"');
  const script='set -Eeuo pipefail\nOPENFGA_DATASTORE_ENGINE=postgres\napp_pid=\nfga_pid=\npostgres_started=false\n'+
    observers+cleanup+'\nstack_phase bootstrap-start\nexit 17\n';
  const result=spawnSync('bash',[],{input:script,encoding:'utf8',timeout:3000});
  assert.equal(result.status,17,result.stderr);
  const lines=result.stdout.trim().split('\n');
  assert.match(lines[0],/^STACK\|phase\|/);
  assert.match(lines[1],/^STACK\|entrypoint-exit\|/);
  const observation=JSON.parse(lines[1].slice('STACK|entrypoint-exit|'.length));
  assert.equal(observation.exit_code,17);
  assert.equal(observation.phase,'bootstrap-start');
  assert.equal(observation.reason,'entrypoint-command-failed');
  assert.equal(observation.app_state,'not-started');
  assert.equal(observation.fga_state,'not-started');
});
