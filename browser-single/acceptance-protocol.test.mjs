// Negative controls for browser evidence framing. These are not VM acceptance.
import assert from 'node:assert/strict';
import test from 'node:test';
import {consumeGuestLine, initialEvidence, requestVerification} from './pages/acceptance-protocol.mjs';

const expectedStatuses = [403,204,200,204,403];
const result = (id, statuses=expectedStatuses) => `LAB|result|${id}|${JSON.stringify({accepted:true,statuses})}`;
const start = id => `LAB|verification-start|${id}`;
function ready() {
  const evidence = initialEvidence('protocol-test-fixture');
  consumeGuestLine(evidence,'LAB|boot');
  consumeGuestLine(evidence,'LAB|ready');
  return evidence;
}

test('each sequential result requires its own requested start and exact response sequence', () => {
  const evidence = ready();
  consumeGuestLine(evidence,start(1));
  consumeGuestLine(evidence,result(1));
  const first = structuredClone(evidence.passes[0]);
  requestVerification(evidence);
  assert.equal(evidence.accepted,false);
  consumeGuestLine(evidence,start(2));
  consumeGuestLine(evidence,result(2));
  assert.equal(evidence.checks,2);
  assert.deepEqual(evidence.passes,[first,{accepted:true,statuses:[...expectedStatuses],id:2}]);
});

test('a duplicate first result cannot pass the second check', () => {
  const evidence = ready();
  consumeGuestLine(evidence,start(1));
  consumeGuestLine(evidence,result(1));
  assert.throws(()=>consumeGuestLine(evidence,result(1)),/repeated/);
  requestVerification(evidence);
  consumeGuestLine(evidence,start(2));
  assert.throws(()=>consumeGuestLine(evidence,result(1)),/repeated/);
  assert.equal(evidence.checks,1);
  assert.equal(evidence.accepted,false);
});

test('startup, readiness, request and start markers cannot be skipped or repeated', () => {
  assert.throws(()=>consumeGuestLine(initialEvidence('fixture'),'LAB|ready'),/readiness/);
  assert.throws(()=>consumeGuestLine(ready(),result(1)),/Unexpected/);
  assert.throws(()=>consumeGuestLine(ready(),start(2)),/Unexpected/);
  const evidence = ready();
  assert.throws(()=>consumeGuestLine(evidence,'LAB|boot'),/second guest boot/);
  assert.throws(()=>consumeGuestLine(evidence,'LAB|ready'),/readiness/);
  consumeGuestLine(evidence,start(1));
  assert.throws(()=>consumeGuestLine(evidence,start(1)),/Unexpected/);
  consumeGuestLine(evidence,result(1));
  assert.throws(()=>consumeGuestLine(evidence,start(2)),/Unexpected/);
  requestVerification(evidence);
  assert.throws(()=>requestVerification(evidence),/No completed/);
});

test('wrong statuses, accepted=false, malformed JSON and unindexed results fail', () => {
  for (const value of [result(1,[200,204,200,204,403]),result(1,[403,204,200,204]),
    'LAB|result|1|{"accepted":false,"statuses":[403,204,200,204,403]}',
    'LAB|result|1|{invalid', 'LAB|result|{"accepted":true,"statuses":[403,204,200,204,403]}']) {
    const evidence = ready();
    consumeGuestLine(evidence,start(1));
    assert.throws(()=>consumeGuestLine(evidence,value));
    assert.equal(evidence.checks,0);
    assert.equal(evidence.accepted,false);
  }
});

test('failed startup or verification is terminal and later results are ignored', () => {
  for (const value of ['LAB|boot-failed','LAB|verification-failed','LAB|boot-failed|openfga','LAB|service-failed|postgres']) {
    const evidence = ready();
    consumeGuestLine(evidence,start(1));
    assert.throws(()=>consumeGuestLine(evidence,value),/failed/);
    // The launcher records failures and clears acceptance before rendering them.
    evidence.error = value;
    consumeGuestLine(evidence,result(1));
    assert.equal(evidence.checks,0);
    assert.equal(evidence.accepted,false);
  }
});
