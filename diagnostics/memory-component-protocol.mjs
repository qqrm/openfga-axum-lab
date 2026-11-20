// Validate recorded real OpenFGA operations; this is not Axum acceptance.
import assert from 'node:assert/strict';

export const pinnedModelSha = 'a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d';
const operations = ['deny-before','grant','allow','revoke','deny-after'];
const forbiddenProcesses = new Set(['postgres','postmaster','initdb','pg_ctl','createdb','app']);
const ulid = /^[0-9A-HJKMNP-TV-Z]{26}$/;

// JSON.parse accepts duplicate keys. Evidence must not hide conflicting values.
export function parseStrictJson(text) {
  const value = JSON.parse(text);
  let cursor = 0;
  const whitespace = ()=>{while (/\s/.test(text[cursor]??'') && cursor<text.length) cursor++;};
  const string = ()=>{
    const begin=cursor++;
    while (cursor<text.length) {
      if (text[cursor]==='\\') cursor+=2;
      else if (text[cursor++]==='"') return JSON.parse(text.slice(begin,cursor));
    }
    throw new Error('Unterminated JSON string');
  };
  const visit = (depth=0)=>{
    assert(depth<32,'JSON nesting exceeded evidence budget');
    whitespace();
    if (text[cursor]==='{') {
      cursor++;whitespace();const keys=new Set();
      if (text[cursor]==='}') {cursor++;return;}
      while (true) {
        whitespace();const key=string();
        assert(!keys.has(key),'Duplicate JSON key: '+key);keys.add(key);
        whitespace();assert.equal(text[cursor++],':');visit(depth+1);whitespace();
        if (text[cursor++]==='}') return;
      }
    }
    if (text[cursor]==='[') {
      cursor++;whitespace();
      if (text[cursor]===']') {cursor++;return;}
      while (true) {visit(depth+1);whitespace();if(text[cursor++]===']')return;}
    }
    if (text[cursor]==='"') {string();return;}
    while(cursor<text.length&&!/[\s,}\]]/.test(text[cursor]))cursor++;
  };
  visit();whitespace();assert.equal(cursor,text.length);
  return value;
}

export function decodeBase64(value, maxBytes = 16384) {
  assert.equal(typeof value, 'string', 'Missing base64 evidence');
  assert(/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value), 'Invalid base64 evidence');
  const bytes = Buffer.from(value, 'base64');
  assert(bytes.length <= maxBytes, 'Oversized evidence field');
  assert.equal(bytes.toString('base64'), value, 'Noncanonical base64 evidence');
  return bytes;
}

export function parseHttpResponse(raw) {
  assert(Buffer.isBuffer(raw) && raw.length > 0 && raw.length <= 16384, 'Invalid raw HTTP response size');
  const split = raw.indexOf('\r\n\r\n');
  assert(split >= 0, 'Incomplete HTTP headers');
  const lines = raw.subarray(0, split).toString('latin1').split('\r\n');
  const status = /^HTTP\/1\.[01] ([0-9]{3})(?: [\x20-\x7e]*)?$/.exec(lines.shift());
  assert(status, 'Invalid HTTP status line');
  const headers = {};
  for (const line of lines) {
    const field = /^([!#$%&'*+.^_`|~0-9A-Za-z-]+):[ \t]*([^\r\n]*)$/.exec(line);
    assert(field, 'Invalid/folded HTTP header');
    const key = field[1].toLowerCase();
    assert(!Object.hasOwn(headers, key), 'Duplicate HTTP header: '+key);
    headers[key] = field[2].trim();
  }
  assert(!Object.hasOwn(headers, 'transfer-encoding'), 'Unsupported HTTP transfer encoding');
  assert(!Object.hasOwn(headers, 'content-encoding'), 'Unsupported HTTP content encoding');
  const body = raw.subarray(split + 4);
  if (Object.hasOwn(headers, 'content-length')) {
    assert(/^(0|[1-9][0-9]*)$/.test(headers['content-length']), 'Invalid Content-Length');
    assert.equal(body.length, Number(headers['content-length']), 'Truncated or extra HTTP body bytes');
  }
  assert(/^application\/json(?:\s*;|$)/i.test(headers['content-type'] ?? ''), 'Expected JSON content type');
  const text = body.toString('utf8');
  assert.deepEqual(Buffer.from(text, 'utf8'), body, 'Invalid UTF-8 HTTP body');
  const json = parseStrictJson(text);
  assert(json !== null && typeof json === 'object' && !Array.isArray(json), 'Expected a JSON response object');
  return {status:Number(status[1]),headers,bodyBytes:body.length,json};
}

export function validateComponentEvents(events) {
  assert(Array.isArray(events), 'Expected component events');
  const of = type => events.filter(event => event.event === type);
  assert.equal(of('failed').length, 0, 'Guest reported a component failure');
  assert.equal(of('boot').length, 1, 'Expected one component boot');
  assert.equal(of('cpu').length, 1, 'Expected one CPU observation');
  assert.equal(of('bootstrap').length, 1, 'Expected one actual model bootstrap');
  assert.equal(of('done').length, 1, 'Expected one component completion');
  const boot = of('boot')[0].data;
  assert.equal(boot.datastore, 'memory');
  assert.equal(boot.virtual_cpus, 2);
  const cpu = of('cpu')[0].data;
  assert.equal(cpu.online_cpus, '0-1');
  assert.equal(cpu.allowed_cpus, '0-1');
  const binding = of('bootstrap')[0].data;
  assert(ulid.test(binding.store_id), 'Invalid store identity');
  assert(ulid.test(binding.model_id), 'Invalid model identity');
  assert.equal(binding.model_sha256, pinnedModelSha);
  const configText = decodeBase64(binding.config_b64, 1024).toString('utf8');
  const config = {store_id:binding.store_id,model_id:binding.model_id,model_sha256:pinnedModelSha};
  assert.equal(configText, JSON.stringify(config), 'Bootstrap configuration bytes differ from the pinned exact schema');

  const processSnapshots = of('processes');
  assert.deepEqual(processSnapshots.map(event=>event.data.phase), ['before-bootstrap','after-cycles']);
  for (const {data} of processSnapshots) {
    assert(Number.isSafeInteger(data.openfga_pid) && data.openfga_pid > 0, 'Invalid service pid');
    const snapshot = decodeBase64(data.snapshot_b64, 65536).toString('utf8');
    const rows = snapshot.trimEnd().split('\n').filter(Boolean).map(line=>line.split('\t'));
    assert.equal(rows.length, data.records, 'Process snapshot record count differs');
    assert.equal(data.forbidden_count, 0, 'Guest reported an unwanted database/application process');
    let ownOpenFga = 0;
    for (const row of rows) {
      assert.equal(row.length, 4, 'Invalid process snapshot row');
      const [pid,ppid,comm,exe] = row;
      assert(/^[0-9]+$/.test(pid) && /^[0-9]+$/.test(ppid), 'Invalid process identity');
      assert(!forbiddenProcesses.has(comm), 'Unexpected running process '+comm);
      assert(!forbiddenProcesses.has(exe.split('/').at(-1)), 'Unexpected running executable '+exe);
      if (Number(pid) === data.openfga_pid) {
        assert.equal(comm, 'openfga');
        assert.equal(exe, '/tmp/browser-lab-runtime/openfga');
        ownOpenFga++;
      }
    }
    assert.equal(ownOpenFga, 1, 'Expected the real staged OpenFGA process');
  }

  const requests = of('request');
  assert.equal(requests.length, 10, 'Expected ten real operations');
  const verified = [];
  for (let i=0;i<requests.length;i++) {
    const data = requests[i].data;
    const cycle = Math.floor(i/5)+1, operation = operations[i%5];
    const user = 'user:browser-memory-probe-'+cycle;
    const object = 'lakekeeper_table:browser-memory-probe-'+cycle;
    const tuple = {user,relation:'ownership',object};
    assert.equal(data.cycle, cycle);
    assert.equal(data.operation, operation);
    assert.equal(data.user, user);
    assert.equal(data.object, object);
    assert.equal(data.relation, 'ownership');
    assert.equal(data.transport_exit, 0, 'Incomplete or timed-out HTTP transport');
    assert.equal(data.budget_seconds, 5, 'Changed client request deadline');
    const request = parseStrictJson(decodeBase64(data.request_body_b64, 4096).toString('utf8'));
    const isCheck = ['deny-before','allow','deny-after'].includes(operation);
    assert.equal(data.method, 'POST');
    assert.equal(data.path, '/stores/'+binding.store_id+(isCheck?'/check':'/write'));
    const expectedRequest = isCheck
      ? {authorization_model_id:binding.model_id,tuple_key:tuple,consistency:'HIGHER_CONSISTENCY'}
      : {authorization_model_id:binding.model_id,[operation==='grant'?'writes':'deletes']:{tuple_keys:[tuple]}};
    assert.deepEqual(request, expectedRequest, 'Actual prepared request differs from the declared fixture/model');
    const raw = decodeBase64(data.response_b64);
    assert.equal(raw.length, data.response_bytes);
    const response = parseHttpResponse(raw);
    assert.equal(response.status, 200, 'OpenFGA returned an HTTP failure');
    if (isCheck) {
      assert(Object.keys(response.json).every(key=>['allowed','resolution'].includes(key)), 'Unknown Check response fields');
      if (Object.hasOwn(response.json,'resolution')) assert(response.json.resolution===null||typeof response.json.resolution==='string','Invalid Check resolution');
      assert.equal(response.json.allowed, operation==='allow', 'Incorrect real authorization decision');
    } else assert.deepEqual(response.json, {}, 'Unexpected real Write response');
    verified.push({cycle,operation,user,object,relation:'ownership',httpStatus:response.status,
      ...(isCheck?{allowed:response.json.allowed}:{}),responseBody:response.json,
      hostReceivedAt:requests[i].hostReceivedAt,guestSeconds:data.guest_seconds});
  }
  const done = of('done')[0].data;
  assert.equal(done.cycles, 2);
  assert.equal(done.requests, 10);
  const ordered = events.map(event=>event.event);
  assert(ordered.indexOf('boot') < ordered.indexOf('cpu'));
  assert(ordered.indexOf('cpu') < ordered.indexOf('bootstrap'));
  assert(ordered.indexOf('bootstrap') < ordered.indexOf('request'));
  assert(ordered.lastIndexOf('request') < ordered.lastIndexOf('processes'));
  assert(ordered.lastIndexOf('processes') < ordered.indexOf('done'));
  assert.equal(ordered.at(-1), 'done', 'Unexpected component activity after terminal success');
  return {binding:config,cycles:2,requests:verified,scope:'direct ownership in the pinned model with the real memory-backed OpenFGA engine',productionAcceptance:false};
}
