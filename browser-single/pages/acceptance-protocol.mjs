// Validate the guest's framing only. Authorization is performed by lab-tool in
// the real Linux guest; this module cannot produce an acceptance result.
export const expectedStatuses = Object.freeze([403, 204, 200, 204, 403]);

export function initialEvidence(source) {
  return {booted: false, ready: false, accepted: false, checks: 0, passes: [], source};
}

export function requestVerification(evidence) {
  if (evidence.error || !evidence.accepted || evidence.pending || evidence.requested) {
    throw new Error('No completed guest check is available to repeat');
  }
  evidence.accepted = false;
  evidence.requested = evidence.checks + 1;
}

export function consumeGuestLine(evidence, value) {
  if (evidence.error) return;
  if (value.startsWith('LAB|boot-failed') || value.startsWith('LAB|verification-failed') ||
      value.startsWith('LAB|service-failed')) {
    throw new Error(value);
  }
  if (value === 'LAB|boot') {
    if (evidence.booted) throw new Error('Unexpected second guest boot');
    evidence.booted = true;
    return 'boot';
  }
  if (value === 'LAB|ready') {
    if (!evidence.booted || evidence.ready) throw new Error('Unexpected guest readiness marker');
    evidence.ready = true;
    return 'ready';
  }
  if (value.startsWith('LAB|verification-start')) {
    const match = /^LAB\|verification-start\|([1-9][0-9]*)$/.exec(value);
    const id = Number(match?.[1]);
    if (!Number.isSafeInteger(id) || !evidence.ready || evidence.pending ||
        id !== evidence.checks + 1 || (id > 1 && evidence.requested !== id)) {
      throw new Error('Unexpected guest verification start');
    }
    evidence.accepted = false;
    evidence.pending = id;
    delete evidence.requested;
    return 'start';
  }
  if (value.startsWith('LAB|result')) {
    const match = /^LAB\|result\|([1-9][0-9]*)\|(.*)$/.exec(value);
    const id = Number(match?.[1]);
    if (!Number.isSafeInteger(id) || !evidence.ready || evidence.pending !== id) {
      throw new Error('Unexpected or repeated guest acceptance result');
    }
    const result = JSON.parse(match[2]);
    if (result?.accepted !== true || JSON.stringify(result.statuses) !== JSON.stringify(expectedStatuses)) {
      throw new Error('Unexpected guest acceptance result');
    }
    evidence.passes.push({...result, id});
    evidence.checks = id;
    evidence.statuses = [...result.statuses];
    evidence.accepted = true;
    delete evidence.pending;
    return 'result';
  }
}
