// Synthetic verifier boundary tests. These do not run or prove a browser VM.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {pinnedModelSha,parseStrictJson,parseHttpResponse,validateComponentEvents,createComponentRecordParser} from './memory-component-protocol.mjs';
const base64=value=>Buffer.from(value).toString('base64');
const http=(body,status=200,extra='')=>Buffer.from(`HTTP/1.0 ${status} Result\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(body)}\r\n${extra}\r\n${body}`);
function fixture() {
  const config={store_id:'01J00000000000000000000001',model_id:'01J00000000000000000000002',model_sha256:pinnedModelSha};
  const event=(event,data)=>({event,data,hostReceivedAt:'2026-11-20T00:00:00.000Z'});
  const snapshot=phase=>event('processes',{phase,records:1,forbidden_count:0,openfga_pid:42,
    snapshot_b64:base64('42\t1\topenfga\t/tmp/browser-lab-runtime/openfga\n')});
  const events=[event('boot',{datastore:'memory',virtual_cpus:2}),event('cpu',{online_cpus:'0-1',allowed_cpus:'0-1'}),
    snapshot('before-bootstrap'),event('bootstrap',{...config,config_b64:base64(JSON.stringify(config))})];
  for (const cycle of [1,2]) for (const operation of ['deny-before','grant','allow','revoke','deny-after']) {
    const tuple={user:'user:browser-memory-probe-'+cycle,relation:'ownership',object:'lakekeeper_table:browser-memory-probe-'+cycle};
    const check=['deny-before','allow','deny-after'].includes(operation);
    const request=check?{authorization_model_id:config.model_id,tuple_key:tuple,consistency:'HIGHER_CONSISTENCY'}
      :{authorization_model_id:config.model_id,[operation==='grant'?'writes':'deletes']:{tuple_keys:[tuple]}};
    const raw=http(JSON.stringify(check?{allowed:operation==='allow'}:{}));
    events.push(event('request',{cycle,operation,...tuple,method:'POST',path:'/stores/'+config.store_id+(check?'/check':'/write'),
      request_body_b64:base64(JSON.stringify(request)),response_b64:raw.toString('base64'),response_bytes:raw.length,
      transport_exit:0,budget_seconds:5,guest_seconds:cycle}));
  }
  events.push(snapshot('after-cycles'),event('done',{cycles:2,requests:10}));
  return events;
}
const requests=events=>events.filter(event=>event.event==='request');
function response(event,body,status=200,extra='') {
  const raw=http(body,status,extra);event.data.response_b64=raw.toString('base64');event.data.response_bytes=raw.length;
}
test('two distinct complete real-operation records are accepted only as a component',()=>{
  const result=validateComponentEvents(fixture());
  assert.equal(result.cycles,2);assert.equal(result.requests.length,10);assert.equal(result.productionAcceptance,false);
});
for (const [label,mutation] of [
  ['deny became allow',events=>response(requests(events)[0],'{"allowed":true}')],
  ['grant did not permit access',events=>response(requests(events)[2],'{"allowed":false}')],
  ['revoke did not remove access',events=>response(requests(events)[4],'{"allowed":true}')],
  ['RPC returned HTTP500',events=>response(requests(events)[0],'{"code":4004}',500)],
  ['valid response arrived after transport timeout',events=>{requests(events)[0].data.transport_exit=124;}],
  ['wrong store endpoint',events=>{requests(events)[0].data.path='/stores/01J00000000000000000000003/check';}],
  ['model changed',events=>{const e=requests(events)[0];const body=JSON.parse(Buffer.from(e.data.request_body_b64,'base64'));body.authorization_model_id='01J00000000000000000000003';e.data.request_body_b64=base64(JSON.stringify(body));}],
  ['second cycle reused fixture',events=>{requests(events)[5].data.user='user:browser-memory-probe-1';}],
  ['unknown Check field',events=>response(requests(events)[0],'{"allowed":false,"unexpected":true}')],
  ['conflicting duplicate Check key',events=>response(requests(events)[2],'{"allowed":false,"allowed":true}')],
  ['incomplete HTTP body',events=>{const e=requests(events)[0],raw=http('{"allowed":false}').subarray(0,-1);e.data.response_b64=raw.toString('base64');e.data.response_bytes=raw.length;}],
  ['ambiguous HTTP framing',events=>response(requests(events)[0],'{"allowed":false}',200,'Content-Length: 17\r\n')],
  ['unsupported chunked HTTP',events=>response(requests(events)[0],'{"allowed":false}',200,'Transfer-Encoding: chunked\r\n')],
  ['PostgreSQL was running',events=>{const e=events.find(event=>event.event==='processes');e.data.records=2;e.data.snapshot_b64=base64(Buffer.from(e.data.snapshot_b64,'base64')+'43\t1\tpostgres\t-\n');}],
  ['one required operation missing',events=>{events.splice(events.indexOf(requests(events)[9]),1);}],
  ['guest failure after transport completion',events=>{events.push({event:'failed',data:{reason:'service-exited'}});}],
]) test(label,()=>{const events=fixture();mutation(events);assert.throws(()=>validateComponentEvents(events));});
test('duplicate request JSON keys, including escaped equivalents, are rejected',()=>{
  assert.throws(()=>parseStrictJson('{"allowed":false,"\\u0061llowed":true}'));
  assert.throws(()=>parseStrictJson('{"writes":{"tuple_keys":[],"tuple_keys":[]}}'));
  assert.deepEqual(parseStrictJson('{"nested":[{},[],1,-2.3e4,true,false,null,"a\\\"b"]}'),
    {nested:[{},[],1,-2.3e4,true,false,null,'a"b']});
});
test('HTTP close-delimited JSON and optional typed resolution are understood',()=>{
  const raw=Buffer.from('HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{"allowed":false}');
  assert.equal(parseHttpResponse(raw).json.allowed,false);
  const events=fixture();response(requests(events)[0],'{"resolution":null,"allowed":false}');
  assert.equal(validateComponentEvents(events).cycles,2);
});

const telemetry='[browser-qemu] {"event":"registered","phase":"add_function","core_id":0}\n';
const cpuLine='FGA_MEMORY|cpu|{"online_cpus":"0-1","allowed_cpus":"0-1","guest_seconds":1}\r\n';
function parseChunks(texts) {
  const parser=createComponentRecordParser();
  const events=texts.flatMap((text,i)=>parser.append({sequence:i+1,text}));
  return {events,capture:parser.snapshot()};
}
test('actual failed-run source chunks reconstruct the CPU record without changing its JSON',()=>{
  const recorded=JSON.parse(readFileSync(new URL('./fixtures/openfga-memory-telemetry-interleaving.json',import.meta.url)));
  assert.equal(recorded.texts.length,recorded.lastSourceSequence-recorded.firstSourceSequence+1);
  assert(recorded.texts.join('').includes('"guest_seconds":1}[browser-qemu] '));
  const {events,capture}=parseChunks(recorded.texts);
  assert.deepEqual(events,[{event:'cpu',data:{online_cpus:'0-1',allowed_cpus:'0-1',guest_seconds:1},sourceSequence:recorded.texts.length}]);
  assert.equal(capture.telemetryChunks,1);
  assert.equal(capture.partialComponentRecord,false);
});
test('whole telemetry chunks may interrupt every byte boundary, including inside JSON and CRLF',()=>{
  for (let boundary=0;boundary<=cpuLine.length;boundary++) {
    const texts=[...cpuLine.slice(0,boundary),telemetry,...cpuLine.slice(boundary)];
    const {events,capture}=parseChunks(texts);
    assert.equal(events.length,1);
    assert.deepEqual(events[0].data,{online_cpus:'0-1',allowed_cpus:'0-1',guest_seconds:1});
    assert.equal(capture.chunks,texts.length);assert.equal(capture.telemetryChunks,1);
    assert.equal(capture.partialComponentRecord,false);
  }
});
test('telemetry is never stripped from component JSON string content',()=>{
  const data={phase:'quoted [browser-qemu] {"event":"text"} and \\ text'};
  const {events,capture}=parseChunks([...'FGA_MEMORY|stage|'+JSON.stringify(data)+'\n']);
  assert.deepEqual(events[0].data,data);assert.equal(capture.telemetryChunks,0);
});
test('mixed, split, malformed and duplicate-key telemetry cannot be silently removed',()=>{
  for (const texts of [
    [telemetry+cpuLine],
    [telemetry.slice(0,5),telemetry.slice(5)],
    [telemetry.slice(0,-2),telemetry.slice(-2)],
    ['[browser-qemu] {"event":"a","event":"b"}\n'],
    ['[browser-qemu] []\n'],
    ['[browser-qemu] {"event":1}\n'],
    ['[browser-qemu] {"event":"a"} trailing\n'],
  ]) assert.throws(()=>parseChunks(texts));
});
test('component payload remains strict after telemetry demultiplexing',()=>{
  for (const line of [
    'FGA_MEMORY|cpu|{"x":1} trailing\n',
    'FGA_MEMORY|cpu|{"x":1,"x":2}\n',
    'FGA_MEMORY|unknown|{}\n',
    'prefix FGA_MEMORY|done|{}\n',
    'FGA_MEMORY|cpu|{}[browser-qemu] {"event":"registered"}\n',
  ]) assert.throws(()=>parseChunks([...line]));
});
test('source gaps and unterminated component records are observable',()=>{
  const parser=createComponentRecordParser();
  assert.throws(()=>parser.append({sequence:2,text:cpuLine}));
  assert.equal(parseChunks([...cpuLine.slice(0,-1)]).capture.partialComponentRecord,true);
  assert.equal(parseChunks(['FGA_MEMORY|done|{"cycles":2,"requests":10}\n','FGA_MEMORY|failed|{']).capture.partialComponentRecord,true);
});
