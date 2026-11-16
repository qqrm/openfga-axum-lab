// Preserve each original log chunk independently of the bounded UI transcript.
export const guestLogEvent = 'browser-lab-log';
export const guestLogFormat = 'browser-lab-log-batches-v1';

export function createTranscriptLog(render, emit, maxChars = 120000) {
  let transcript = '';
  let sequence = 0;
  return text => {
    emit({sequence: ++sequence, text});
    transcript = (transcript + text).slice(-maxChars);
    render(transcript);
  };
}

// One pump delivers arrays, not one binding/Promise chain entry per UART byte.
// Both queue bounds include the batch awaiting the host's acknowledgement.
export function subscribeToGuestLog(target, deliver, {maxPendingChunks = 65536,
  maxPendingBytes = 4 * 1024 * 1024, maxBatchChunks = 1024, onError = () => {}} = {}) {
  if (!Number.isSafeInteger(maxBatchChunks) || maxBatchChunks < 1) throw new Error('Invalid guest log batch limit');
  let pending = Promise.resolve();
  let pumping = false;
  const queue = [];
  const sizes = [];
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
  const pump = async () => {
    try {
      while (queue.length) {
        const batch = queue.splice(0, maxBatchChunks);
        const size = sizes.splice(0, batch.length).reduce((sum, value) => sum + value, 0);
        await deliver(batch);
        pendingChunks -= batch.length;
        pendingBytes -= size;
      }
    } catch (failure) {
      stopWithError(failure);
      // A failed delivery makes the prefix incomplete. Do not send later
      // sequences past an unacknowledged batch or retain an abandoned queue.
      queue.length = sizes.length = 0;
      pendingChunks = pendingBytes = 0;
      throw failure;
    } finally { pumping = false; }
  };
  const listener = event => {
    try {
      const {sequence, text} = event.detail ?? {};
      if (sequence !== lastSequence + 1 || typeof text !== 'string') throw new Error('Invalid guest log chunk sequence or text');
      const size = encoder.encode(text).byteLength;
      if (pendingChunks >= maxPendingChunks || pendingBytes + size > maxPendingBytes) {
        throw new Error('Guest log delivery queue exceeded its capture limit');
      }
      queue.push({sequence, text});
      sizes.push(size);
      pendingChunks++;
      pendingBytes += size;
      lastSequence = sequence;
      if (!pumping) {
        pumping = true;
        pending = Promise.resolve().then(pump);
        pending.catch(() => {});
      }
    } catch (failure) {
      stopWithError(failure);
    }
  };
  target.addEventListener(guestLogEvent, listener);
  return async () => {
    target.removeEventListener(guestLogEvent, listener);
    await pending;
    if (error) throw error;
    return lastSequence;
  };
}

// Freeze before awaiting delivery. The caller can retrieve this cutoff even
// when draining rejects or times out; the drain result is a separate promise.
export function freezeGuestLogCapture(target) {
  const cutoff = {expectedSequence: target.labLogSequence, guest: structuredClone(target.labEvidence)};
  let drain;
  try {
    if (typeof target.__finishBrowserLabLog !== 'function') throw new Error('Guest log capture was not installed');
    drain = target.__finishBrowserLabLog();
  } catch (error) { drain = Promise.reject(error); }
  target.__browserLabLogDrain = Promise.resolve(drain).then(
    deliveredSequence => ({deliveredSequence}), error => ({error: String(error)}));
  return cutoff;
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
// v1 JSONL rows are {firstSequence, receivedAt, texts}. Each source chunk is
// exactly {sequence: firstSequence + index, text: texts[index]}. This preserves
// empty chunks and UTF-16 surrogate boundaries without per-character objects
// or repeated timestamps. receivedAt is the batch's host receipt time.
export function createGuestLogCollector({maxBytes = 16 * 1024 * 1024, maxChunks = 1000000,
  now = () => new Date().toISOString()} = {}) {
  const records = [];
  const encoder = new TextEncoder();
  let bytes = 0;
  let chunks = 0;
  let error;
  const appendBatch = batch => {
    if (error) throw error;
    try {
      if (!Array.isArray(batch) || !batch.length) throw new Error('Invalid guest log batch');
      let size = 0;
      for (const [index, chunk] of batch.entries()) {
        if (!chunk || chunk.sequence !== chunks + index + 1 || typeof chunk.text !== 'string') {
          throw new Error('Guest log stream has an invalid or missing sequence');
        }
        size += encoder.encode(chunk.text).byteLength;
      }
      if (bytes + size > maxBytes || chunks + batch.length > maxChunks) throw new Error('Guest log stream exceeded its capture limit');
      // Validate the complete batch before committing any of it.
      records.push({firstSequence: chunks + 1, receivedAt: now(), texts: batch.map(chunk => chunk.text)});
      chunks += batch.length;
      bytes += size;
    } catch (failure) { error = failure; throw failure; }
  };
  return {
    append: chunk => appendBatch([chunk]),
    appendBatch,
    snapshot(expectedSequence) {
      const complete = !error && Number.isSafeInteger(expectedSequence)
        && expectedSequence >= 0 && expectedSequence === chunks;
      return {text: records.map(record => record.texts.join('')).join(''),
        jsonl: records.map(record => JSON.stringify(record) + '\n').join(''),
        metadata: {format: guestLogFormat, complete, chunks, batches: records.length, bytes, expectedSequence,
          ...(error ? {error: String(error)} : {})}};
    }
  };
}
