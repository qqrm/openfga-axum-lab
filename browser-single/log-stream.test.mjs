import assert from 'node:assert/strict';
import test from 'node:test';
import {createTranscriptLog, createGuestLogCollector, guestLogEvent,
  subscribeToGuestLog, drainGuestLogWithin} from './pages/log-stream.mjs';

test('full chunks survive UI truncation, including identical successive tails', () => {
  const capture = createGuestLogCollector({now: () => 'host-receipt-time'});
  let visible;
  const log = createTranscriptLog(value => { visible = value; }, capture.append);
  const chunk = 'a'.repeat(150000);
  log(chunk);
  log(chunk);
  log('\nLAB|boot\n');
  const result = capture.snapshot(3);
  assert.equal(visible.length, 120000);
  assert.equal(result.text, chunk + chunk + '\nLAB|boot\n');
  assert.equal(result.metadata.complete, true);
  assert.equal(result.metadata.chunks, 3);
  const records = result.jsonl.trimEnd().split('\n').map(line => JSON.parse(line));
  assert.deepEqual(records.map(record => record.sequence), [1, 2, 3]);
  assert.ok(records.every(record => record.receivedAt === 'host-receipt-time'));
});

test('sequence gaps, duplicates and missing final chunks cannot pass', () => {
  for (const bad of [{sequence: 3, text: 'gap'}, {sequence: 1, text: 'duplicate'},
    {sequence: 2, text: 42}]) {
    const capture = createGuestLogCollector();
    capture.append({sequence: 1, text: 'first'});
    assert.throws(() => capture.append(bad), /invalid or missing sequence/);
    assert.equal(capture.snapshot(1).metadata.complete, false);
    assert.equal(capture.snapshot(1).text, 'first');
  }
  const capture = createGuestLogCollector();
  capture.append({sequence: 1, text: 'first'});
  assert.equal(capture.snapshot(2).metadata.complete, false);
  assert.equal(capture.snapshot(undefined).metadata.complete, false);
});

test('a failed UI render cannot silently lose an emitted chunk', () => {
  const capture = createGuestLogCollector();
  const failure = new Error('UI unavailable');
  const log = createTranscriptLog(() => { throw failure; }, capture.append);
  assert.throws(() => log('retained before rendering'), error => error === failure);
  assert.equal(capture.snapshot(1).text, 'retained before rendering');
  assert.equal(capture.snapshot(1).metadata.complete, true);
});

test('UTF-8 capture limit fails explicitly and retains earlier evidence', () => {
  const capture = createGuestLogCollector({maxBytes: 4});
  capture.append({sequence: 1, text: '🙂'});
  assert.throws(() => capture.append({sequence: 2, text: 'x'}), /capture limit/);
  const result = capture.snapshot(2);
  assert.equal(result.text, '🙂');
  assert.equal(result.metadata.bytes, 4);
  assert.equal(result.metadata.complete, false);
});

test('empty chunks cannot bypass the host record limit', () => {
  const capture = createGuestLogCollector({maxChunks: 1});
  capture.append({sequence: 1, text: ''});
  assert.throws(() => capture.append({sequence: 2, text: ''}), /capture limit/);
  assert.equal(capture.snapshot(2).metadata.complete, false);
});

test('subscription delivers in order, drains pending chunks and then detaches', async () => {
  const target = new EventTarget();
  const capture = createGuestLogCollector();
  const finish = subscribeToGuestLog(target, async chunk => {
    await Promise.resolve();
    capture.append(chunk);
  });
  const send = (sequence, text) => target.dispatchEvent(new CustomEvent(guestLogEvent,
    {detail: {sequence, text}}));
  send(1, 'first');
  send(2, 'second');
  const drained = finish();
  send(3, 'after stop');
  assert.equal(await drained, 2);
  assert.equal(capture.snapshot(2).text, 'firstsecond');
  assert.equal(capture.snapshot(2).metadata.complete, true);
});

test('delivery failure is retained and rejects the final drain', async () => {
  const target = new EventTarget();
  const failure = new Error('host capture unavailable');
  const finish = subscribeToGuestLog(target, async () => { throw failure; });
  target.dispatchEvent(new CustomEvent(guestLogEvent, {detail: {sequence: 1, text: 'first'}}));
  await assert.rejects(finish(), error => error === failure);
});

test('browser queue limits stop subscription and preserve the accepted prefix', async () => {
  for (const options of [{maxPendingChunks: 1}, {maxPendingBytes: 4}]) {
    const target = new EventTarget();
    const capture = createGuestLogCollector();
    let reported;
    const finish = subscribeToGuestLog(target, chunk => capture.append(chunk),
      {...options, onError: error => { reported = error; }});
    target.dispatchEvent(new CustomEvent(guestLogEvent, {detail: {sequence: 1, text: '🙂'}}));
    target.dispatchEvent(new CustomEvent(guestLogEvent, {detail: {sequence: 2, text: 'x'}}));
    target.dispatchEvent(new CustomEvent(guestLogEvent, {detail: {sequence: 3, text: 'detached'}}));
    await assert.rejects(finish(), /delivery queue exceeded/);
    assert.match(String(reported), /delivery queue exceeded/);
    assert.equal(capture.snapshot(3).text, '🙂');
    assert.equal(capture.snapshot(3).metadata.complete, false);
  }
});

test('a host timer bounds a permanently stuck browser drain', async () => {
  const target = new EventTarget();
  const capture = createGuestLogCollector();
  const finish = subscribeToGuestLog(target, chunk => chunk.sequence === 1
    ? capture.append(chunk) : new Promise(() => {}));
  target.dispatchEvent(new CustomEvent(guestLogEvent, {detail: {sequence: 1, text: 'prefix'}}));
  target.dispatchEvent(new CustomEvent(guestLogEvent, {detail: {sequence: 2, text: 'stuck'}}));
  await assert.rejects(drainGuestLogWithin(finish(), 10), /drain timed out/);
  assert.equal(capture.snapshot(2).text, 'prefix');
  assert.equal(capture.snapshot(2).metadata.complete, false);
});
