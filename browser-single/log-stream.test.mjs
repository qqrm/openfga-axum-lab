import assert from 'node:assert/strict';
import test from 'node:test';
import {createTranscriptLog, createCoalescedTranscriptRender, createGuestLogCollector, guestLogEvent,
  subscribeToGuestLog, drainGuestLogWithin, freezeGuestLogCapture, guestLogFormat} from './pages/log-stream.mjs';

const decode = jsonl => jsonl.trimEnd().split('\n').filter(Boolean).flatMap(line => {
  const row = JSON.parse(line);
  return row.texts.map((text,index) => ({sequence: row.firstSequence + index, text, receivedAt: row.receivedAt}));
});
const send = (target,sequence,text) => target.dispatchEvent(new CustomEvent(guestLogEvent,{detail:{sequence,text}}));

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
  assert.equal(result.metadata.format,guestLogFormat);
  const records = decode(result.jsonl);
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

test('subscription delivers batched chunks in order, drains and then detaches', async () => {
  const target = new EventTarget();
  const capture = createGuestLogCollector();
  const finish = subscribeToGuestLog(target, async batch => {
    await Promise.resolve();
    capture.appendBatch(batch);
  });
  send(target,1,'first');
  send(target,2,'second');
  const drained = finish();
  send(target,3,'after stop');
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
    const finish = subscribeToGuestLog(target, batch => capture.appendBatch(batch),
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
  const finish = subscribeToGuestLog(target, batch => batch[0].sequence === 1
    ? capture.appendBatch(batch) : new Promise(() => {}), {maxBatchChunks:1});
  target.dispatchEvent(new CustomEvent(guestLogEvent, {detail: {sequence: 1, text: 'prefix'}}));
  target.dispatchEvent(new CustomEvent(guestLogEvent, {detail: {sequence: 2, text: 'stuck'}}));
  await assert.rejects(drainGuestLogWithin(finish(), 10), /drain timed out/);
  assert.equal(capture.snapshot(2).text, 'prefix');
  assert.equal(capture.snapshot(2).metadata.complete, false);
});

test('malformed batches commit none of the batch and leave prior evidence incomplete', () => {
  for (const batch of [null,{},[],[{sequence:2,text:'prefix'},{sequence:4,text:'gap'}],
    [{sequence:2,text:'prefix'},{sequence:3,text:42}]]) {
    const capture=createGuestLogCollector();
    capture.appendBatch([{sequence:1,text:'previous batch'}]);
    assert.throws(()=>capture.appendBatch(batch),/Invalid guest log batch|invalid or missing sequence/);
    assert.equal(capture.snapshot(1).metadata.complete,false);
    assert.equal(capture.snapshot(1).text,'previous batch');
    assert.equal(capture.snapshot(1).metadata.chunks,1);
  }
});

test('120000 per-character UART chunks survive bursts with one delivery per 1024 chunks', async () => {
  const target=new EventTarget(),capture=createGuestLogCollector();
  let deliveries=0,largest=0;
  const finish=subscribeToGuestLog(target,async batch=>{
    deliveries++;largest=Math.max(largest,batch.length);
    capture.appendBatch(batch);
  });
  for(let burst=0;burst<3;burst++) {
    for(let i=1;i<=40000;i++)send(target,burst*40000+i,String.fromCharCode(32+(i%95)));
    await new Promise(resolve=>setImmediate(resolve));
  }
  assert.equal(await finish(),120000);
  // A partial tail may join the next burst before its 16 ms timer fires.
  assert(deliveries >= Math.ceil(120000/1024));
  assert(deliveries <= 3*Math.ceil(40000/1024));
  assert.equal(largest,1024);
  const result=capture.snapshot(120000),source=decode(result.jsonl);
  assert.equal(result.metadata.complete,true);
  assert.equal(source.length,120000);
  source.forEach((chunk,index)=>{
    assert.equal(chunk.sequence,index+1);
    assert.equal(chunk.text,String.fromCharCode(32+((index%40000+1)%95)));
  });
  assert.equal(result.text,source.map(chunk=>chunk.text).join(''));
  assert.equal(result.metadata.batches,deliveries);
});

test('new chunks arriving during a delivery remain ordered and only one batch is in flight', async () => {
  const target=new EventTarget(),capture=createGuestLogCollector();
  let release,started,active=0,maximum=0;
  const gate=new Promise(resolve=>{release=resolve;});
  const firstStarted=new Promise(resolve=>{started=resolve;});
  const finish=subscribeToGuestLog(target,async batch=>{
    active++;maximum=Math.max(maximum,active);
    if(batch[0].sequence===1){started();await gate;}
    capture.appendBatch(batch);active--;
  },{maxBatchChunks:2});
  send(target,1,'a');send(target,2,'b');send(target,3,'c');
  await firstStarted;
  send(target,4,'d');send(target,5,'e');
  const drained=finish();release();
  assert.equal(await drained,5);
  assert.equal(maximum,1);
  assert.equal(capture.snapshot(5).text,'abcde');
  assert.deepEqual(decode(capture.snapshot(5).jsonl).map(chunk=>chunk.sequence),[1,2,3,4,5]);
});

test('queue limits include the unacknowledged in-flight batch', async () => {
  const target=new EventTarget(),capture=createGuestLogCollector();
  let release,started;
  const gate=new Promise(resolve=>{release=resolve;});
  const firstStarted=new Promise(resolve=>{started=resolve;});
  const finish=subscribeToGuestLog(target,async batch=>{started();await gate;capture.appendBatch(batch);},
    {maxBatchChunks:1,maxPendingChunks:2});
  send(target,1,'a');await firstStarted;
  send(target,2,'b');send(target,3,'overflow');
  const drained=finish();release();
  await assert.rejects(drained,/delivery queue exceeded/);
  assert.equal(capture.snapshot(3).text,'ab');
  assert.equal(capture.snapshot(3).metadata.complete,false);
});

test('compact batches retain empty chunks and split UTF-16 surrogates exactly', () => {
  const capture=createGuestLogCollector();
  capture.appendBatch([{sequence:1,text:''},{sequence:2,text:'\ud83d'}]);
  capture.appendBatch([{sequence:3,text:'\ude42'},{sequence:4,text:'€'}]);
  const result=capture.snapshot(4);
  assert.deepEqual(decode(result.jsonl).map(({sequence,text})=>({sequence,text})),[
    {sequence:1,text:''},{sequence:2,text:'\ud83d'},{sequence:3,text:'\ude42'},{sequence:4,text:'€'}]);
  assert.equal(result.text,'🙂€');
  assert.equal(result.metadata.bytes,9); // Per-chunk encoding counts two replacement characters.
  assert.equal(new TextEncoder().encode(result.text).byteLength,7);
  assert.equal(result.metadata.complete,true);
});

test('cutoff keeps synchronous guest state and expected sequence when draining fails', async () => {
  for(const finish of [()=>{throw new Error('sync drain failure');},()=>Promise.reject(new Error('async drain failure'))]) {
    const target={labLogSequence:12,labEvidence:{booted:true,ready:false,accepted:false},__finishBrowserLabLog:finish};
    const cutoff=freezeGuestLogCapture(target);
    target.labEvidence.booted=false;
    assert.deepEqual(cutoff,{expectedSequence:12,guest:{booted:true,ready:false,accepted:false}});
    assert.match((await target.__browserLabLogDrain).error,/drain failure/);
  }
});

test('cutoff survives a bounded timeout of a permanently pending delivery', async () => {
  const target={labLogSequence:7,labEvidence:{booted:true},__finishBrowserLabLog:()=>new Promise(()=>{})};
  const cutoff=freezeGuestLogCapture(target);
  await assert.rejects(drainGuestLogWithin(target.__browserLabLogDrain,10),/drain timed out/);
  assert.deepEqual(cutoff,{expectedSequence:7,guest:{booted:true}});
});

test('visible rendering coalesces separate tasks without delaying original emissions', async t => {
  t.mock.timers.enable({apis:['setTimeout']});
  const rendered = [], emitted = [];
  const renderer = createCoalescedTranscriptRender(text => rendered.push(text), assert.fail);
  const log = createTranscriptLog(renderer.render, chunk => emitted.push(chunk));
  log('first');
  await new Promise(resolve => setImmediate(resolve));
  log('second');
  t.mock.timers.tick(15);
  assert.deepEqual(rendered, []);
  await new Promise(resolve => setImmediate(resolve));
  log('🙂');
  assert.deepEqual(emitted, [
    {sequence:1,text:'first'}, {sequence:2,text:'second'}, {sequence:3,text:'🙂'}]);
  t.mock.timers.tick(1);
  assert.deepEqual(rendered, ['firstsecond🙂']);
  log('latest');
  renderer.flush();
  assert.deepEqual(rendered, ['firstsecond🙂', 'firstsecond🙂latest']);
  t.mock.timers.tick(100);
  assert.equal(rendered.length, 2);
});

test('a render failure reports once, stays observable and never schedules a retry loop', t => {
  t.mock.timers.enable({apis:['setTimeout']});
  const capture = createGuestLogCollector();
  const failure = new Error('render sentinel');
  let attempts = 0, reports = 0, log;
  const renderer = createCoalescedTranscriptRender(() => { attempts++; throw failure; }, error => {
    assert.equal(error, failure);
    reports++;
    log('error report');
  });
  log = createTranscriptLog(renderer.render, capture.append);
  log('first');
  assert.throws(renderer.flush, error => error === failure);
  log('later');
  t.mock.timers.tick(100);
  assert.equal(attempts, 1);
  assert.equal(reports, 1);
  assert.equal(capture.snapshot(3).text, 'firsterror reportlater');
  assert.throws(renderer.flush, error => error === failure);
});

test('partial capture batches coalesce separate tasks and full batches wake the timer', async t => {
  t.mock.timers.enable({apis:['setTimeout']});
  const target = new EventTarget(), capture = createGuestLogCollector(), batches = [];
  const finish = subscribeToGuestLog(target, batch => {
    batches.push(batch);
    capture.appendBatch(batch);
  }, {maxBatchChunks:4});
  send(target,1,'');
  await new Promise(resolve => setImmediate(resolve));
  send(target,2,'\ud83d');
  await new Promise(resolve => setImmediate(resolve));
  send(target,3,'\ude42');
  t.mock.timers.tick(15);
  await Promise.resolve();
  assert.equal(batches.length, 0);
  send(target,4,'€');
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(batches.map(batch => batch.length), [4]);
  send(target,5,'tail');
  await new Promise(resolve => setImmediate(resolve));
  t.mock.timers.tick(15);
  await Promise.resolve();
  assert.equal(batches.length, 1);
  t.mock.timers.tick(1);
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(batches.map(batch => batch.length), [4,1]);
  assert.equal(await finish(), 5);
  const result = capture.snapshot(5);
  assert.equal(result.text, '🙂€tail');
  assert.equal(result.metadata.bytes, 13);
  assert.equal(result.metadata.complete, true);
  assert.deepEqual(decode(result.jsonl).map(({sequence,text}) => ({sequence,text})), [
    {sequence:1,text:''}, {sequence:2,text:'\ud83d'}, {sequence:3,text:'\ude42'},
    {sequence:4,text:'€'}, {sequence:5,text:'tail'}]);
});

test('finish immediately detaches and drains a final partial batch without its timer', async t => {
  t.mock.timers.enable({apis:['setTimeout']});
  const target = new EventTarget(), capture = createGuestLogCollector();
  let deliveries = 0;
  const finish = subscribeToGuestLog(target, batch => {
    deliveries++;
    capture.appendBatch(batch);
  });
  send(target,1,'first');
  await new Promise(resolve => setImmediate(resolve));
  send(target,2,'second');
  const drained = finish();
  send(target,3,'after cutoff');
  assert.equal(await drained, 2);
  assert.equal(capture.snapshot(2).text, 'firstsecond');
  assert.equal(capture.snapshot(2).metadata.complete, true);
  t.mock.timers.tick(100);
  await Promise.resolve();
  assert.equal(deliveries, 1);
});

test('a pending render failure at cutoff rejects completeness even after a full batch was delivered', async t => {
  t.mock.timers.enable({apis:['setTimeout']});
  const target = new EventTarget(), capture = createGuestLogCollector();
  const failure = new Error('cutoff render sentinel');
  target.labLogSequence = 0;
  target.labEvidence = {ready:true,accepted:true};
  let reports = 0;
  const renderer = createCoalescedTranscriptRender(() => { throw failure; }, error => {
    reports++;
    target.labEvidence = {ready:false,accepted:false,error:String(error)};
  });
  target.__flushBrowserLabTranscript = renderer.flush;
  target.__finishBrowserLabLog = subscribeToGuestLog(target, capture.appendBatch, {maxBatchChunks:1});
  const log = createTranscriptLog(renderer.render, chunk => {
    target.labLogSequence = chunk.sequence;
    send(target,chunk.sequence,chunk.text);
  });
  log('one full capture batch');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(capture.snapshot(1).metadata.complete, true);
  assert.equal(reports, 0);
  const cutoff = freezeGuestLogCapture(target);
  assert.deepEqual(cutoff, {expectedSequence:1,
    guest:{ready:false,accepted:false,error:String(failure)}});
  assert.deepEqual(await target.__browserLabLogDrain, {error:String(failure)});
  assert.equal(reports, 1);
  assert.throws(renderer.flush, error => error === failure);
  t.mock.timers.tick(100);
  assert.equal(reports, 1);
});

test('cutoff flushes the latest visible transcript before taking the guest snapshot', async t => {
  t.mock.timers.enable({apis:['setTimeout']});
  const target = new EventTarget(), capture = createGuestLogCollector();
  target.labLogSequence = 0;
  target.labEvidence = {rendered:''};
  const renderer = createCoalescedTranscriptRender(text => { target.labEvidence.rendered = text; }, assert.fail);
  target.__flushBrowserLabTranscript = renderer.flush;
  target.__finishBrowserLabLog = subscribeToGuestLog(target, capture.appendBatch);
  const log = createTranscriptLog(renderer.render, chunk => {
    target.labLogSequence = chunk.sequence;
    send(target,chunk.sequence,chunk.text);
  });
  log('first'); log('latest');
  const cutoff = freezeGuestLogCapture(target);
  assert.deepEqual(cutoff, {expectedSequence:2,guest:{rendered:'firstlatest'}});
  assert.deepEqual(await target.__browserLabLogDrain, {deliveredSequence:2});
  log('after cutoff');
  assert.equal(capture.snapshot(2).text, 'firstlatest');
  assert.deepEqual(cutoff.guest, {rendered:'firstlatest'});
  renderer.flush();
});
