import assert from 'node:assert/strict';
import test from 'node:test';
import {beginCommand, consumeConceptLine, createSerialLineDecoder, initialConceptEvidence, parseStrictJson} from './pages/protocol.mjs';

const model = 'a'.repeat(64);
const health = {protocol: 1, document: 'demo', owner: 'alice',
  store_id: '01M7VM9J2FBK9XMSYYBW8J6C5S', model_id: '01M7VJXV0BTTT847JBH9K2RBWS',
  model_sha256: model, title: 'Browser authorization demo'};
const frame = (kind, value) => 'CONCEPT|' + kind + '|' + JSON.stringify(value);
function ready() {
  const state = initialConceptEvidence();
  state.expectedModelSha256 = model;
  consumeConceptLine(state, frame('ready', health));
  return state;
}
function response(state, overrides = {}) {
  return {id: state.pending.id, operation: state.pending.operation, actor: state.pending.actor,
    http_status: 200, body: {actor: state.pending.actor, document: 'demo', can_read: false, can_share: false}, elapsed_ms: 14, ...overrides};
}

test('readiness binds the real backend model and never invents permissions', () => {
  const state = initialConceptEvidence();
  assert.throws(() => beginCommand(state, 'read', 'bob'), /not ready/);
  assert.throws(() => consumeConceptLine(state, frame('ready', health)), /verified startup model/);
  state.expectedModelSha256 = model;
  consumeConceptLine(state, frame('phase', {phase: 'postgres', detail: 'Starting storage'}));
  assert.equal(state.ready, false);
  consumeConceptLine(state, frame('ready', health));
  assert.equal(state.ready, true);
  assert.deepEqual(state.observations, {alice: null, bob: null});
  assert.equal(state.document, null);
  assert.throws(() => consumeConceptLine(state, frame('ready', health)), /second backend/);
});

test('one request is in flight and a response must match all correlation fields', () => {
  for (const mismatch of [{id: 2}, {id: '1'}, {actor: 'alice'}, {operation: 'grant'}]) {
    const state = ready();
    assert.match(beginCommand(state, 'permissions', 'bob'), /^CONCEPT\|command\|\{"id":1,"operation":"permissions","actor":"bob"\}\n$/);
    assert.throws(() => beginCommand(state, 'read', 'alice'), /current request/);
    assert.throws(() => consumeConceptLine(state, frame('response', response(state, mismatch))), /does not match/);
    assert.equal(state.responses.length, 0);
    assert.equal(state.pending.id, 1);
  }
});

test('terminal command echo cannot stand in for an actual HTTP response', () => {
  const state = ready();
  const command = beginCommand(state, 'permissions', 'bob');
  assert.equal(consumeConceptLine(state, command.trim()).kind, 'echo');
  assert.equal(state.pending.id, 1);
  assert.equal(state.responses.length, 0);
  assert.equal(state.observations.bob, null);
  const observed = response(state);
  consumeConceptLine(state, frame('response', observed));
  assert.equal(state.pending, null);
  assert.equal(state.observations.bob.can_read, false);
  assert.throws(() => consumeConceptLine(state, frame('response', observed)), /without a pending/);
  assert.throws(() => consumeConceptLine(state, frame('command', {id: 99, operation: 'read', actor: 'bob'})), /Unrecognized command echo/);
});

test('grant and revoke clear stale observations; HTTP 204 alone never grants a badge', () => {
  const state = ready();
  beginCommand(state, 'permissions', 'bob');
  consumeConceptLine(state, frame('response', response(state)));
  beginCommand(state, 'grant', 'alice');
  assert.deepEqual(state.observations, {alice: null, bob: null});
  consumeConceptLine(state, frame('response', response(state, {http_status: 204, body: null})));
  assert.deepEqual(state.observations, {alice: null, bob: null});
  beginCommand(state, 'permissions', 'bob');
  consumeConceptLine(state, frame('response', response(state, {body: {actor: 'bob', document: 'demo', can_read: true, can_share: false}})));
  assert.equal(state.observations.bob.can_read, true);
  beginCommand(state, 'revoke', 'alice');
  consumeConceptLine(state, frame('response', response(state, {http_status: 204, body: null})));
  assert.equal(state.observations.bob, null);
});

test('a denied Bob mutation is a real HTTP observation, not a protocol failure', () => {
  const state = ready();
  beginCommand(state, 'grant', 'bob');
  consumeConceptLine(state, frame('response', response(state, {http_status: 403, body: {error: 'forbidden'}})));
  assert.equal(state.ready, true);
  assert.equal(state.responses[0].http_status, 403);
  assert.deepEqual(state.observations, {alice: null, bob: null});
  beginCommand(state, 'read', 'bob');
  consumeConceptLine(state, frame('response', response(state, {http_status: 403, body: {error: 'forbidden'}})));
  assert.equal(state.document, null);
  assert.equal(state.lastRead.http_status, 403);
});

test('transport failures and incomplete bodies preserve uncertainty even with an HTTP status', () => {
  for (const status of [null, 200, 204, 503]) {
    const state = ready();
    beginCommand(state, 'permissions', 'alice');
    consumeConceptLine(state, frame('response', response(state, {http_status: status, body: null, error: 'body incomplete'})));
    assert.equal(state.observations.alice, null);
    assert.equal(state.responses[0].http_status, status);
    assert.equal(state.responses[0].error, 'body incomplete');
  }
  const state = ready();
  beginCommand(state, 'read', 'bob');
  assert.throws(() => consumeConceptLine(state, frame('response', response(state, {http_status: null, body: null}))), /transport error/);
});

test('typed permission/document responses cannot accept fabricated or malformed success', () => {
  for (const body of [{actor: 'bob', document: 'demo', can_read: 'true', can_share: false},
    {actor: 'alice', document: 'demo', can_read: true, can_share: true},
    {actor: 'bob', document: 'other', can_read: true, can_share: false}]) {
    const state = ready(); beginCommand(state, 'permissions', 'bob');
    assert.throws(() => consumeConceptLine(state, frame('response', response(state, {body}))), /permission observation/);
  }
  const state = ready(); beginCommand(state, 'read', 'alice');
  consumeConceptLine(state, frame('response', response(state, {body: {id: 'demo', title: 'Real document', body: 'Actual HTTP response text'}})));
  assert.equal(state.document.body, 'Actual HTTP response text');
  assert.equal(state.document.actor, 'alice');
  beginCommand(state, 'grant', 'alice');
  assert.throws(() => consumeConceptLine(state, frame('response', response(state, {http_status: 204, body: {}}))), /empty HTTP 204/);
});

test('duplicate keys, trailing/mixed records and guest failures are rejected', () => {
  assert.throws(() => parseStrictJson('{"http_status":200,"http_status":403}'), /Duplicate/);
  assert.throws(() => parseStrictJson('{"body":{"allowed":true,"allowed":false}}'), /Duplicate/);
  const state = ready();
  assert.equal(consumeConceptLine(state, '[browser-qemu] {"event":"registered"}'), null);
  assert.throws(() => consumeConceptLine(state, frame('phase', {phase: 'ready'}) + '[browser-qemu] {}'));
  assert.throws(() => consumeConceptLine(state, frame('error', {error: 'invalid_command_or_sequence'})), /invalid_command_or_sequence/);
  assert.throws(() => consumeConceptLine(state, frame('failed', {message: 'A required service exited'})), /required service exited/);
});

test('UART decoder preserves fragmented UTF-8 and CRLF; no partial line is accepted', () => {
  const records = [];
  const decoder = createSerialLineDecoder(value => records.push(value));
  const bytes = new TextEncoder().encode('CONCEPT|phase|{"phase":"boot","detail":"A → B"}\r\n');
  for (const byte of bytes.subarray(0, -1)) decoder.write([byte]);
  assert.deepEqual(records, []);
  assert(decoder.pendingBytes > 0);
  decoder.write(bytes.subarray(-1));
  assert.deepEqual(records, ['CONCEPT|phase|{"phase":"boot","detail":"A → B"}']);
  assert.equal(decoder.pendingBytes, 0);
  assert.throws(() => createSerialLineDecoder(() => {}, 3).write(new TextEncoder().encode('abcd')), /framing limit/);
  assert.throws(() => createSerialLineDecoder(() => {}).write([255]), /encoded data/);
});
