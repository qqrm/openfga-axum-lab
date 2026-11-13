// Preserve each original log chunk independently of the bounded UI transcript.
export const guestLogEvent = 'browser-lab-log';

export function createTranscriptLog(render, emit, maxChars = 120000) {
  let transcript = '';
  let sequence = 0;
  return text => {
    emit({sequence: ++sequence, text});
    transcript = (transcript + text).slice(-maxChars);
    render(transcript);
  };
}

// The browser test installs this before starting the VM. A single promise chain
// preserves delivery order and lets the host drain all accepted chunks at exit.
export function subscribeToGuestLog(target, deliver, {maxPendingChunks = 4096,
  maxPendingBytes = 4 * 1024 * 1024, onError = () => {}} = {}) {
  let pending = Promise.resolve();
  let lastSequence = 0;
  let pendingChunks = 0;
  let pendingBytes = 0;
  let error;
  const encoder = new TextEncoder();
  const stopWithError = failure => {
    if (error) return;
    error = failure;
    target.removeEventListener(guestLogEvent, listener);
    Promise.resolve().then(() => onError(failure)).catch(() => {});
  };
  const listener = event => {
    const chunk = event.detail;
    if (!chunk || typeof chunk.text !== 'string') {
      stopWithError(new Error('Invalid guest log chunk'));
      return;
    }
    const size = encoder.encode(chunk.text).byteLength;
    if (pendingChunks >= maxPendingChunks || pendingBytes + size > maxPendingBytes) {
      stopWithError(new Error('Guest log delivery queue exceeded its capture limit'));
      return;
    }
    pendingChunks++;
    pendingBytes += size;
    lastSequence = chunk.sequence;
    pending = pending.then(() => deliver(chunk)).catch(failure => {
      stopWithError(failure);
      throw failure;
    }).finally(() => {
      pendingChunks--;
      pendingBytes -= size;
    });
    pending.catch(() => {});
  };
  target.addEventListener(guestLogEvent, listener);
  return async () => {
    target.removeEventListener(guestLogEvent, listener);
    await pending;
    if (error) throw error;
    return lastSequence;
  };
}

// A host timer also works when the page event loop or the delivery is stuck.
export async function drainGuestLogWithin(operation, timeoutMs = 10000) {
  let timer;
  try {
    return await Promise.race([operation, new Promise((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error('Guest log drain timed out')), timeoutMs);
    })]);
  } finally { clearTimeout(timer); }
}

// Host-side collection is bounded too, but overflow or a sequence gap is an
// explicit test failure. It must never silently produce a truncated "complete"
// transcript. Timestamps describe host receipt, not guest execution time.
export function createGuestLogCollector({maxBytes = 16 * 1024 * 1024, maxChunks = 100000,
  now = () => new Date().toISOString()} = {}) {
  const records = [];
  const encoder = new TextEncoder();
  let bytes = 0;
  let error;
  return {
    append(chunk) {
      if (error) throw error;
      try {
        if (!chunk || chunk.sequence !== records.length + 1 || typeof chunk.text !== 'string') {
          throw new Error('Guest log stream has an invalid or missing sequence');
        }
        const size = encoder.encode(chunk.text).byteLength;
        if (bytes + size > maxBytes || records.length >= maxChunks) {
          throw new Error('Guest log stream exceeded its capture limit');
        }
        records.push({sequence: chunk.sequence, receivedAt: now(), text: chunk.text});
        bytes += size;
      } catch (failure) {
        error = failure;
        throw failure;
      }
    },
    snapshot(expectedSequence) {
      const complete = !error && Number.isSafeInteger(expectedSequence)
        && expectedSequence >= 0 && expectedSequence === records.length;
      return {text: records.map(record => record.text).join(''),
        jsonl: records.map(record => JSON.stringify(record) + '\n').join(''),
        metadata: {complete, chunks: records.length, bytes, expectedSequence,
          ...(error ? {error: String(error)} : {})}};
    }
  };
}
