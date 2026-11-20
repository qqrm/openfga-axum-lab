// Framing and observations only. All authorization decisions come from the
// real Axum/OpenFGA services behind the guest's HTTP bridge.
export const actors = Object.freeze(['alice', 'bob']);
export const operations = Object.freeze(['permissions', 'read', 'grant', 'revoke']);
export const maxFrameBytes = 65536;
const maxResponses = 512;
const ulid = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const sha256 = /^[0-9a-f]{64}$/;

function require(condition, message) {
  if (!condition) throw new Error(message);
}

function object(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function boundedString(value, max = 4096) {
  return typeof value === 'string' && value.length > 0 && value.length <= max;
}

// Reject duplicate keys instead of letting a later value conceal an earlier
// status, actor or permission. JSON.parse first validates the complete syntax.
export function parseStrictJson(text) {
  const value = JSON.parse(text);
  let cursor = 0;
  const whitespace = () => { while (cursor < text.length && /\s/.test(text[cursor])) cursor++; };
  const string = () => {
    const begin = cursor++;
    while (cursor < text.length) {
      if (text[cursor] === '\\') cursor += 2;
      else if (text[cursor++] === '"') return JSON.parse(text.slice(begin, cursor));
    }
    throw new Error('Unterminated protocol string');
  };
  const visit = (depth = 0) => {
    require(depth < 16, 'Protocol JSON nesting limit exceeded');
    whitespace();
    if (text[cursor] === '{') {
      cursor++; whitespace();
      const keys = new Set();
      if (text[cursor] === '}') { cursor++; return; }
      while (true) {
        whitespace();
        const key = string();
        require(!keys.has(key), 'Duplicate protocol key: ' + key);
        keys.add(key);
        whitespace(); require(text[cursor++] === ':', 'Invalid protocol object');
        visit(depth + 1); whitespace();
        if (text[cursor++] === '}') return;
      }
    }
    if (text[cursor] === '[') {
      cursor++; whitespace();
      if (text[cursor] === ']') { cursor++; return; }
      while (true) { visit(depth + 1); whitespace(); if (text[cursor++] === ']') return; }
    }
    if (text[cursor] === '"') { string(); return; }
    while (cursor < text.length && !/[\s,}\]]/.test(text[cursor])) cursor++;
  };
  visit(); whitespace();
  require(cursor === text.length, 'Trailing protocol JSON');
  return value;
}

export function initialConceptEvidence() {
  return {
    protocol: 1, scenario: 'document-permissions', started: false, ready: false,
    failed: false, expectedModelSha256: null, health: null, phase: null, phases: [],
    commandEchoes: 0,
    pending: null, nextCommandId: 1, responses: [],
    observations: {alice: null, bob: null}, document: null, lastRead: null,
  };
}

export function beginCommand(state, operation, actor, now = () => new Date().toISOString()) {
  require(state.ready && !state.failed, 'The backend is not ready');
  require(!state.pending, 'Wait for the current request to finish');
  require(operations.includes(operation) && actors.includes(actor), 'Unknown demonstration command');
  require(state.responses.length < maxResponses, 'This session reached its observation limit. Reset to start again.');
  require(Number.isSafeInteger(state.nextCommandId) && state.nextCommandId > 0, 'Invalid command sequence');
  const command = {id: state.nextCommandId++, operation, actor};
  state.pending = {...command, sentAt: now()};
  if (operation === 'permissions') state.observations[actor] = null;
  if (operation === 'read') { state.document = null; state.lastRead = null; }
  if (operation === 'grant' || operation === 'revoke') {
    // A write might succeed even when its response is lost. Until a fresh
    // observation arrives, do not display either the old or an assumed state.
    state.observations = {alice: null, bob: null};
    state.document = null; state.lastRead = null;
  }
  return 'CONCEPT|command|' + JSON.stringify(command) + '\n';
}

function validateHealth(value, state) {
  require(object(value) && value.protocol === 1 && value.document === 'demo' && value.owner === 'alice',
    'Unexpected backend identity');
  require(ulid.test(value.store_id ?? '') && ulid.test(value.model_id ?? ''), 'Missing real OpenFGA store/model identity');
  require(sha256.test(state.expectedModelSha256 ?? '') && value.model_sha256 === state.expectedModelSha256,
    'The backend model differs from the verified startup model');
  require(boundedString(value.title, 256), 'Missing document title');
  if (value.actors !== undefined) require(JSON.stringify(value.actors) === JSON.stringify(actors), 'Unexpected demo actors');
}

function validateResponse(value, pending) {
  require(object(value) && pending, 'Unexpected response without a pending request');
  require(Number.isSafeInteger(value.id) && value.id > 0 && value.id === pending.id &&
    value.actor === pending.actor && value.operation === pending.operation, 'Response does not match the pending request');
  require(value.http_status === null || (Number.isInteger(value.http_status) && value.http_status >= 100 && value.http_status <= 599),
    'Invalid observed HTTP status');
  require(Object.hasOwn(value, 'body'), 'Missing observed HTTP body');
  require(Number.isSafeInteger(value.elapsed_ms) && value.elapsed_ms >= 0, 'Invalid bridge duration');
  if (value.error !== undefined) {
    require(boundedString(value.error) && value.body === null, 'Invalid bridge error');
    return; // A known status plus a broken body is an observation, not success.
  }
  require(value.http_status !== null, 'A missing HTTP response must include its transport error');
  if (value.http_status < 200 || value.http_status >= 300) {
    require(value.body === null || object(value.body), 'Unexpected HTTP error body');
    return;
  }
  if (value.operation === 'permissions') {
    require(value.http_status === 200 && object(value.body) && value.body.actor === value.actor &&
      value.body.document === 'demo' && typeof value.body.can_read === 'boolean' && typeof value.body.can_share === 'boolean',
    'Invalid permission observation');
  } else if (value.operation === 'read') {
    require(value.http_status === 200 && object(value.body) && value.body.id === 'demo' &&
      boundedString(value.body.title, 256) && typeof value.body.body === 'string' && value.body.body.length <= 32768,
    'Invalid document response');
  } else {
    require(value.http_status === 204 && value.body === null, 'A completed viewer mutation must return an empty HTTP 204');
  }
}

export function consumeConceptLine(state, line, now = () => new Date().toISOString()) {
  if (!line.startsWith('CONCEPT|')) return null;
  require(!state.failed, 'Protocol data arrived after terminal failure');
  const match = /^CONCEPT\|(phase|ready|response|failed|error|command)\|(\{.*\})$/.exec(line);
  require(match, 'Malformed concept protocol record');
  const kind = match[1];
  const value = parseStrictJson(match[2]);
  require(object(value), 'Protocol payload must be an object');
  if (kind === 'command') {
    // Linux may echo terminal input. Only recognize an exact locally issued
    // command, and never let its echo complete a request or change a badge.
    const issued = state.pending?.id === value.id ? state.pending : state.responses.find(item => item.id === value.id);
    require(issued && Object.keys(value).sort().join(',') === 'actor,id,operation' &&
      value.actor === issued.actor && value.operation === issued.operation, 'Unrecognized command echo');
    state.commandEchoes++;
    return {kind: 'echo', value};
  }
  if (kind === 'phase') {
    require(/^[a-z0-9_-]{1,64}$/.test(value.phase ?? ''), 'Invalid boot phase');
    require(value.detail === undefined || boundedString(value.detail, 1024), 'Invalid boot detail');
    state.phase = value;
    require(state.phases.length < 256, 'Too many startup phase records');
    state.phases.push({...value, receivedAt: now()});
    return {kind, value};
  }
  if (kind === 'failed' || kind === 'error') {
    const message = kind === 'failed' ? value.message : value.error;
    require(boundedString(message), 'Invalid guest failure');
    throw new Error(message);
  }
  if (kind === 'ready') {
    require(!state.ready && !state.pending && state.responses.length === 0, 'Unexpected second backend readiness record');
    validateHealth(value, state);
    state.health = value;
    state.ready = true;
    return {kind, value};
  }
  validateResponse(value, state.pending);
  const observed = {...value, sentAt: state.pending.sentAt, receivedAt: now()};
  state.responses.push(observed);
  state.pending = null;
  if (value.error === undefined && value.operation === 'permissions' && value.http_status === 200) {
    state.observations[value.actor] = {...value.body, requestId: value.id, receivedAt: observed.receivedAt};
  }
  if (value.operation === 'read') {
    state.lastRead = observed;
    state.document = value.error === undefined && value.http_status === 200 ? {...value.body, actor: value.actor} : null;
  }
  return {kind, value: observed};
}

// Feed this decoder only bytes from the guest UART, never the merged display
// transcript (which also contains QEMU Module.print/printErr telemetry).
export function createSerialLineDecoder(onLine, limit = maxFrameBytes) {
  const decoder = new TextDecoder('utf-8', {fatal: true});
  let pending = '', bytes = 0;
  return {
    get pendingBytes() { return bytes; },
    write(input) {
      const text = decoder.decode(new Uint8Array(input), {stream: true});
      for (const character of text) {
        if (character === '\n') {
          onLine(pending.replaceAll('\r', ''));
          pending = ''; bytes = 0;
        } else {
          const point = character.codePointAt(0);
          bytes += point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
          require(bytes <= limit, 'Guest serial line exceeded the framing limit');
          pending += character;
        }
      }
      return text;
    },
  };
}
