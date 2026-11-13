// Native-only proof: delay complete, genuine OpenFGA responses without replacing them.
import {createHash} from 'node:crypto';
import {copyFileSync, mkdirSync, readFileSync, renameSync, writeFileSync, chmodSync, realpathSync} from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import {performance} from 'node:perf_hooks';
import {setTimeout as sleep} from 'node:timers/promises';

const ROOT = 'artifacts/native-bootstrap';
const STACK = 'native-bootstrap-stack';
const PROXY = 'native-bootstrap-proxy';
const SOURCE = '207d15fab685d3adb9c5d51a048982c2236ae75c';
const PACKAGING = '4b7a1e1933083232fb8305a60a47bdd3c32e6689';
const MODEL_SHA = 'a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d';
const ULID = '[0-9A-HJKMNP-TV-Z]{26}';
const MODEL_GET = new RegExp('^/stores/' + ULID + '/authorization-models/' + ULID + '$');
const MAX_BODY = 1024 * 1024;
const CHECK_TUPLE = {user: 'user:browser-bootstrap-probe', relation: 'ownership',
  object: 'lakekeeper_table:browser-bootstrap-probe'};
const OWNER_LABEL = 'io.authzlab.native-bootstrap-run';

function requireCondition(ok, message) {
  if (!ok) throw new Error(message);
}

function emit(event, fields = {}) {
  process.stdout.write(JSON.stringify({event, ...fields}) + '\n');
}

function digest(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function saveJson(name, value) {
  writeFileSync(path.join(ROOT, name), JSON.stringify(value, null, 2) + '\n');
}

function operation(method, requestPath) {
  if (requestPath === '/healthz') return 'health';
  if (method === 'POST' && requestPath === '/stores') return 'create_store';
  if (method === 'POST' && new RegExp('^/stores/' + ULID + '/authorization-models$').test(requestPath)) return 'model_post';
  if (method === 'GET' && MODEL_GET.test(requestPath)) return 'model_get';
  if (method === 'GET' && new RegExp('^/stores/' + ULID + '$').test(requestPath)) return 'get_store';
  if (method === 'POST' && new RegExp('^/stores/' + ULID + '/read$').test(requestPath)) return 'read';
  if (method === 'POST' && new RegExp('^/stores/' + ULID + '/check$').test(requestPath)) return 'check';
  throw new Error('Unexpected bootstrap proxy operation');
}

function filteredHeaders(rawHeaders, excluded) {
  const headers = [];
  for (let index = 0; index < rawHeaders.length; index += 2) {
    if (!excluded.includes(rawHeaders[index].toLowerCase())) {
      headers.push(rawHeaders[index], rawHeaders[index + 1]);
    }
  }
  return headers;
}

async function requestBody(request) {
  requireCondition(request.headers['transfer-encoding'] === undefined, 'Unexpected streamed request');
  const lengthText = request.headers['content-length'] ?? '0';
  requireCondition(/^(0|[1-9][0-9]*)$/.test(lengthText), 'Invalid request length');
  const length = Number(lengthText);
  requireCondition(Number.isSafeInteger(length) && length <= MAX_BODY, 'Oversized bootstrap request');
  const chunks = [];
  let received = 0;
  for await (const chunk of request) {
    received += chunk.length;
    requireCondition(received <= length, 'Oversized bootstrap request body');
    chunks.push(chunk);
  }
  requireCondition(received === length, 'Incomplete bootstrap request');
  return Buffer.concat(chunks);
}

function realResponse(request, body) {
  return new Promise((resolve, reject) => {
    const headers = filteredHeaders(request.rawHeaders, ['host', 'connection', 'content-length']);
    headers.push('Host', '127.0.0.1:8080', 'Content-Length', String(body.length));
    const upstream = http.request({
      hostname: '127.0.0.1', port: 8080, method: request.method,
      path: request.url, headers, agent: false
    }, response => {
      const chunks = [];
      let received = 0;
      response.on('data', chunk => {
        received += chunk.length;
        if (received > MAX_BODY) {
          upstream.destroy(new Error('Oversized real OpenFGA response'));
          return;
        }
        chunks.push(chunk);
      });
      response.once('error', reject);
      response.once('end', () => {
        if (!response.complete) {
          reject(new Error('Incomplete real OpenFGA response'));
          return;
        }
        resolve({status: response.statusCode, headers: response.rawHeaders, body: Buffer.concat(chunks)});
      });
    });
    upstream.setTimeout(10_000, () => upstream.destroy(new Error('Real OpenFGA request timed out')));
    upstream.once('error', reject);
    upstream.end(body);
  });
}

function forwardResponse(response, upstream) {
  if (response.destroyed || response.writableEnded) return Promise.resolve(false);
  return new Promise((resolve, reject) => {
    response.once('finish', () => resolve(true));
    response.once('close', () => resolve(false));
    response.once('error', () => resolve(false));
    try {
      const headers = filteredHeaders(upstream.headers, ['connection', 'transfer-encoding', 'content-length']);
      headers.push('Content-Length', String(upstream.body.length), 'Connection', 'close');
      response.sendDate = false;
      response.writeHead(upstream.status, headers);
      response.end(upstream.body);
    } catch (error) {
      reject(error);
    }
  });
}

function serveProxy() {
  let nextId = 0;
  const server = http.createServer((request, response) => {
    const forward = async () => {
      const control = JSON.parse(readFileSync('/native-proof/control.json', 'utf8'));
      const phase = control.phase;
      const op = operation(request.method, request.url);
      const delay = ['model_post', 'model_get', 'check'].includes(op) ? (control[op] ?? 0) : 0;
      requireCondition(delay === 0 || delay === 6, 'Unexpected diagnostic delay');
      const body = await requestBody(request);
      const requestId = ++nextId;
      emit('start', {phase, operation: op, request_id: requestId});
      const upstream = await realResponse(request, body);
      // Start the delay only after the entire real upstream response is available.
      const completed = performance.now();
      const detail = {};
      if (op === 'read' && upstream.status === 200) {
        const decoded = JSON.parse(upstream.body.toString('utf8'));
        const tuples = Object.hasOwn(decoded, 'tuples') ? decoded.tuples : [];
        requireCondition(Array.isArray(tuples), 'Invalid real tuple response');
        detail.tuple_count = tuples.length;
        detail.continuation_token = Object.hasOwn(decoded, 'continuation_token') ? decoded.continuation_token : '';
      }
      if (op === 'check' && upstream.status === 200) {
        const decoded = JSON.parse(upstream.body.toString('utf8'));
        const input = JSON.parse(body.toString('utf8'));
        detail.allowed = decoded.allowed;
        detail.request_store_id = request.url.split('/')[2];
        detail.request_model_id = input.authorization_model_id;
        detail.consistency = input.consistency;
        detail.tuple_key = input.tuple_key;
        detail.request_keys = Object.keys(input).sort();
      }
      const bodySha = digest(upstream.body);
      emit('upstream', {phase, operation: op, request_id: requestId, status: upstream.status,
        body_bytes: upstream.body.length, body_sha256: bodySha, delay_seconds: delay, ...detail});
      await sleep(delay * 1000);
      let forwarded = false;
      try {
        forwarded = await forwardResponse(response, upstream);
      } finally {
        emit('finished', {phase, operation: op, request_id: requestId, status: upstream.status,
          forwarded, body_sha256: bodySha, delay_elapsed_ms: Math.round(performance.now() - completed)});
      }
    };
    forward().catch(error => {
      emit('proxy_error', {message: String(error)});
      response.destroy();
    });
  });
  server.listen(18080, '127.0.0.1', () => {
    emit('proxy_ready', {port: 18080, upstream: '127.0.0.1:8080'});
  });
}

function command(args, {timeout = 15, check = true} = {}) {
  const result = spawnSync(args[0], args.slice(1), {
    encoding: 'utf8', timeout: timeout * 1000, maxBuffer: 32 * 1024 * 1024
  });
  if (result.error) throw result.error;
  requireCondition(result.status !== null, 'Command terminated by signal: ' + result.signal);
  const output = {returncode: result.status, stdout: result.stdout, stderr: result.stderr};
  if (check && output.returncode !== 0) {
    throw new Error('Command failed: ' + args.slice(0, 4).join(' ') + '\n' + output.stderr.slice(-4000));
  }
  return output;
}

function docker(args, options) {
  return command(['docker', ...args], options);
}

function logs() {
  return docker(['logs', PROXY], {check: false}).stdout.split('\n')
    .filter(line => line.startsWith('{')).map(line => JSON.parse(line));
}

async function waitFor(predicate, seconds, description) {
  const end = performance.now() + seconds * 1000;
  while (performance.now() < end) {
    if (predicate()) return;
    const state = docker(['inspect', '-f', '{{.State.Running}}', STACK], {check: false});
    requireCondition(state.returncode === 0 && state.stdout.trim() === 'true', 'Real stack exited: ' + description);
    await sleep(200);
  }
  throw new Error('Timed out: ' + description);
}

function exists(filename) {
  return docker(['exec', STACK, 'test', '-f', filename], {check: false}).returncode === 0;
}

function phase(name, postDelay = 0, getDelay = 0, checkDelay = 0) {
  writeFileSync(path.join(ROOT, 'control.tmp'), JSON.stringify({
    phase: name, model_post: postDelay, model_get: getDelay, check: checkDelay
  }));
  renameSync(path.join(ROOT, 'control.tmp'), path.join(ROOT, 'control.json'));
}

function phaseEvents(name, event) {
  return logs().filter(entry => entry.phase === name && entry.event === event);
}

function counts(operations) {
  const result = {};
  for (const op of operations) result[op] = (result[op] ?? 0) + 1;
  return Object.entries(result).sort(([left], [right]) => left.localeCompare(right));
}

function verifyPhase(name, expected, delayed) {
  const events = logs().filter(entry => entry.phase === name);
  const started = events.filter(entry => entry.event === 'start');
  requireCondition(JSON.stringify(counts(started.map(entry => entry.operation))) === JSON.stringify(counts(expected)),
    'Unexpected request count or retry in ' + name);
  requireCondition(JSON.stringify(started.map(entry => entry.operation)) === JSON.stringify(expected),
    'Unexpected bootstrap operation order in ' + name);
  const upstream = events.filter(entry => entry.event === 'upstream');
  requireCondition(upstream.length === started.length, 'A request did not complete against real OpenFGA');
  for (const entry of upstream) {
    const status = ['create_store', 'model_post'].includes(entry.operation) ? 201 : 200;
    requireCondition(entry.status === status, 'Real OpenFGA did not confirm ' + entry.operation);
  }
  for (const op of delayed) {
    const finished = events.filter(entry => entry.event === 'finished' && entry.operation === op);
    requireCondition(finished.length === 1 && finished[0].delay_elapsed_ms >= 5900,
      'The genuine response was not delayed more than five seconds');
  }
}

function verifyEmptyStoreCheck(name, config) {
  const entries = phaseEvents(name, 'upstream');
  const checks = entries.filter(entry => entry.operation === 'check');
  const reads = entries.filter(entry => entry.operation === 'read');
  requireCondition(checks.length === 1 && checks[0].status === 200 && checks[0].allowed === false,
    'Exactly one real successful deny Check is required in ' + name);
  const check = checks[0];
  requireCondition(JSON.stringify(check.request_keys) === JSON.stringify([
    'authorization_model_id', 'consistency', 'tuple_key'
  ]) && check.consistency === 'HIGHER_CONSISTENCY',
  'The Check must use only the actual model and higher consistency');
  requireCondition(check.tuple_key && Object.keys(check.tuple_key).length === 3 &&
    Object.entries(CHECK_TUPLE).every(([key, value]) => check.tuple_key[key] === value),
  'The real ownership probe tuple differs');
  requireCondition(['request_store_id', 'request_model_id'].every(key =>
    new RegExp('^' + ULID + '$').test(check[key])), 'Invalid real Check binding');
  requireCondition(reads.length === 1 && reads[0].tuple_count === 0 &&
    reads[0].continuation_token === '', 'The Check must follow a confirmed empty tuple store');
  if (config) {
    requireCondition(check.request_store_id === config.store_id &&
      check.request_model_id === config.model_id, 'The actual Check and published binding differ');
  }
  return {request_count: 1, status: check.status, allowed: check.allowed,
    store_id: check.request_store_id, authorization_model_id: check.request_model_id,
    consistency: check.consistency, tuple_key: check.tuple_key, tuple_count_before: reads[0].tuple_count};
}

function cleanupOwned(owned, runId) {
  for (const name of owned) {
    const result = docker(['logs', name], {check: false});
    writeFileSync(path.join(ROOT, name + '.log'), result.stdout + result.stderr);
  }
  if (owned.includes(STACK)) {
    for (const name of ['initdb.log', 'postgres.log']) {
      docker(['cp', STACK + ':/tmp/browser-lab/' + name, path.join(ROOT, name)], {check: false});
    }
  }
  for (const name of [...owned].reverse()) {
    const owner = docker(['inspect', '-f', '{{index .Config.Labels "' + OWNER_LABEL + '"}}', name], {check: false});
    if (owner.returncode === 0 && owner.stdout.trim() === runId) {
      docker(['rm', '-f', '-v', name], {check: false});
    }
  }
}

async function main() {
  // Host-only input: proxy mode deliberately has no dependency on this variable.
  const fgaSha = process.env.BROWSER_BOOTSTRAP_OPENFGA_SHA256 ?? '';
  requireCondition(/^[0-9a-f]{64}$/.test(fgaSha), 'A verified OpenFGA SHA256 is required');
  const runId = process.env.GITHUB_RUN_ID;
  requireCondition(typeof runId === 'string' && runId.length > 0, 'A proof run identity is required');
  mkdirSync(ROOT, {recursive: true});
  const image = readFileSync(path.join(ROOT, 'image-id.txt'), 'utf8').trim();
  requireCondition(/^sha256:[0-9a-f]{64}$/.test(image), 'Invalid immutable image ID');
  const imageInfo = JSON.parse(docker(['image', 'inspect', image]).stdout)[0];
  requireCondition(imageInfo.Architecture === 'amd64' && imageInfo.Os === 'linux', 'Unexpected image target');
  const proofDir = realpathSync(ROOT);
  copyFileSync('packaging/browser-single/entrypoint.sh', path.join(ROOT, 'entrypoint.sh'));
  const gate = path.join(ROOT, 'bootstrap-gate.sh');
  writeFileSync(gate, [
    '#!/usr/bin/env bash', 'set -euo pipefail',
    'test "$#" -eq 1 && test "$1" = bootstrap',
    'touch /tmp/native-bootstrap-ready',
    "timeout 300s bash -c 'until test -f /tmp/native-start-bootstrap; do sleep 0.2; done'",
    'FGA_URL=http://127.0.0.1:18080 BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS=30 BROWSER_BOOTSTRAP_EMPTY_STORE_CHECK=yes /usr/local/bin/lab-tool bootstrap',
    'touch /tmp/native-bootstrap-done',
    "timeout 300s bash -c 'until test -f /tmp/native-start-app; do sleep 0.2; done'", ''
  ].join('\n'));
  chmodSync(gate, 0o555);
  phase('held');
  const owned = [];
  try {
    for (const name of [STACK, PROXY]) {
      requireCondition(docker(['inspect', name], {check: false}).returncode !== 0, 'Container name already exists');
    }
    docker(['run', '-d', '--name', STACK, '--label', OWNER_LABEL + '=' + runId,
      '--shm-size=128m', '--entrypoint', '/bin/bash',
      '-e', 'BROWSER_LAB_EPHEMERAL=yes', '-e', 'OPENFGA_LOG_LEVEL=warn',
      '-e', 'OPENFGA_METRICS_ENABLED=false',
      '-e', 'BROWSER_LAB_OPENFGA_BINARY=/native-openfga/openfga',
      '-e', 'BROWSER_LAB_BOOTSTRAP_BINARY=/native-proof/bootstrap-gate.sh',
      '--mount', 'type=bind,src=' + proofDir + ',dst=/native-proof,readonly',
      '--mount', 'type=bind,src=' + realpathSync('artifacts/native-bootstrap-input') + ',dst=/native-openfga,readonly',
      image, '/native-proof/entrypoint.sh']);
    owned.push(STACK);
    const binaryHash = docker(['exec', STACK, 'sha256sum', '/usr/local/bin/lab-tool']).stdout.split(/\s+/)[0];
    requireCondition(docker(['exec', STACK, 'sha256sum', '/native-openfga/openfga']).stdout.split(/\s+/)[0] === fgaSha,
      'Actual mounted OpenFGA binary changed');
    await waitFor(() => exists('/tmp/native-bootstrap-ready'), 240, 'real PostgreSQL/OpenFGA startup');
    docker(['run', '-d', '--name', PROXY, '--label', OWNER_LABEL + '=' + runId,
      '--network', 'container:' + STACK, '--read-only', '--cap-drop', 'ALL',
      '--security-opt', 'no-new-privileges', '--user', '65534:65534',
      '--mount', 'type=bind,src=' + proofDir + ',dst=/native-proof,readonly',
      'node:24-bookworm-slim', 'node', '/native-proof/native-proof.mjs', 'proxy'], {timeout: 120});
    owned.push(PROXY);
    await waitFor(() => logs().some(entry => entry.event === 'proxy_ready'), 20, 'real-response proxy');
    const defaults = [];
    for (const [name, postDelay, getDelay, expected, delayed] of [
      ['default-post', 6, 0, ['health', 'create_store', 'model_post'], ['model_post']],
      ['default-get', 0, 6, ['health', 'create_store', 'model_post', 'get_store', 'model_get'], ['model_get']]
    ]) {
      phase(name, postDelay, getDelay);
      const config = '/tmp/' + name + '.json';
      const start = performance.now();
      const result = docker(['exec', STACK, 'env', '-u', 'BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS',
        '-u', 'BROWSER_BOOTSTRAP_EMPTY_STORE_CHECK', 'FGA_URL=http://127.0.0.1:18080', 'FGA_CONFIG=' + config, '/usr/local/bin/lab-tool', 'bootstrap'],
      {timeout: 15, check: false});
      const elapsed = (performance.now() - start) / 1000;
      const configPublished = exists(config);
      const responseWithheld = !phaseEvents(name, 'finished').some(entry => delayed.includes(entry.operation));
      const record = {phase: name, exit: result.returncode, elapsed_ms: Math.round(elapsed * 1000),
        config_published: configPublished, response_still_withheld: responseWithheld, stderr: result.stderr.slice(-4000)};
      // Persist the actual CLI result before asserting its meaning.
      saveJson(name + '-result.json', record);
      emit('default_attempt', record);
      requireCondition(result.returncode === 1 && elapsed >= 4.5 && elapsed < 6,
        'Default operation must fail near five seconds, before the six-second response can arrive');
      const typedDeadline = /Elapsed|TimedOut/.test(result.stderr) || /timed out|deadline has elapsed/i.test(result.stderr);
      const wrappedModelGet = name === 'default-get' &&
        result.stderr.includes('OpenFGA target cannot be verified; refusing silent reset: error sending request for url') &&
        new RegExp('http://127[.]0[.]0[.]1:18080/stores/' + ULID + '/authorization-models/' + ULID).test(result.stderr);
      requireCondition(typedDeadline || wrappedModelGet, 'Unexpected error while the real model response was withheld');
      requireCondition(!configPublished, 'Failed bootstrap published a configuration');
      requireCondition(responseWithheld, 'The delayed response finished before the failing CLI result was observed');
      await waitFor(() => delayed.every(op => phaseEvents(name, 'finished').some(entry => entry.operation === op)),
        10, 'completion of delayed real response');
      verifyPhase(name, expected, delayed);
      defaults.push(record);
      emit('default_fail_closed', record);
    }
    // The model override must not enlarge the ordinary Check request budget.
    // Hold an already completed real 200/false response for six seconds.
    phase('check-deadline', 0, 0, 6);
    const checkConfig = '/tmp/check-deadline.json';
    const checkStart = performance.now();
    const checkResult = docker(['exec', STACK, 'env',
      'BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS=30', 'BROWSER_BOOTSTRAP_EMPTY_STORE_CHECK=yes',
      'FGA_URL=http://127.0.0.1:18080', 'FGA_CONFIG=' + checkConfig,
      '/usr/local/bin/lab-tool', 'bootstrap'], {timeout: 15, check: false});
    const checkElapsed = (performance.now() - checkStart) / 1000;
    const checkFailure = {phase: 'check-deadline', exit: checkResult.returncode,
      elapsed_ms: Math.round(checkElapsed * 1000), config_published: exists(checkConfig),
      response_still_withheld: !phaseEvents('check-deadline', 'finished').some(entry => entry.operation === 'check'),
      stderr: checkResult.stderr.slice(-4000)};
    saveJson('check-deadline-result.json', checkFailure);
    emit('check_deadline_attempt', checkFailure);
    requireCondition(checkFailure.exit === 1 && checkElapsed >= 4.5 && checkElapsed < 6 &&
      !checkFailure.config_published && checkFailure.response_still_withheld,
    'Check must fail closed near five seconds, before its real six-second response arrives');
    // Actual timing, upstream decision and absent publication establish the
    // timeout; either Tokio or reqwest may supply the preserved raw stderr.
    checkFailure.real_check = verifyEmptyStoreCheck('check-deadline');
    await waitFor(() => phaseEvents('check-deadline', 'finished').some(entry => entry.operation === 'check'),
      10, 'completion of delayed real deny Check');
    verifyPhase('check-deadline', [
      'health', 'create_store', 'model_post', 'get_store', 'model_get', 'read', 'check'
    ], ['check']);
    saveJson('check-deadline-result.json', checkFailure);
    emit('check_deadline_fail_closed', checkFailure);
    phase('opt-in', 6, 6);
    docker(['exec', STACK, 'touch', '/tmp/native-start-bootstrap']);
    await waitFor(() => exists('/tmp/native-bootstrap-done'), 90, 'opt-in real model provisioning');
    await waitFor(() => ['model_post', 'model_get'].every(op =>
      phaseEvents('opt-in', 'finished').some(entry => entry.operation === op)), 10, 'completed opt-in delayed responses');
    verifyPhase('opt-in', [
      'health', 'create_store', 'model_post', 'get_store', 'model_get', 'read', 'check'
    ], ['model_post', 'model_get']);
    const reads = phaseEvents('opt-in', 'upstream').filter(entry => entry.operation === 'read');
    requireCondition(reads.length === 1 && reads[0].tuple_count === 0 && reads[0].continuation_token === '',
      'The real new store was not proven empty');
    const config = JSON.parse(docker(['exec', STACK, 'cat', '/tmp/browser-lab/config.json']).stdout);
    requireCondition(config.model_sha256 === MODEL_SHA && ['store_id', 'model_id'].every(key =>
      new RegExp('^' + ULID + '$').test(config[key])), 'Published binding identity differs');
    const emptyStoreCheck = verifyEmptyStoreCheck('opt-in', config);
    const tuples = docker(['exec', STACK, 'psql', '-X', '-qAt', '-w', '-v', 'ON_ERROR_STOP=1',
      '-h', '/tmp/browser-lab', '-U', 'authzlab', '-d', 'openfga',
      '-c', 'SELECT count(*) FROM tuple']).stdout.trim();
    requireCondition(tuples === '0', 'A real tuple was written before the original app migration');
    const models = docker(['exec', STACK, 'psql', '-X', '-qAt', '-w', '-v', 'ON_ERROR_STOP=1',
      '-h', '/tmp/browser-lab', '-U', 'authzlab', '-d', 'openfga',
      '-c', 'SELECT count(*) FROM authorization_model']).stdout.trim();
    requireCondition(models === '4', 'Expected the four actual PostgreSQL authorization models');
    phase('passthrough');
    docker(['exec', STACK, 'touch', '/tmp/native-start-app']);
    await waitFor(() => docker(['exec', STACK, '/usr/local/bin/lab-tool', 'probe'], {check: false}).returncode === 0,
      240, 'real Axum readiness and migration');
    const passes = [];
    for (const number of [1, 2]) {
      const result = docker(['exec', '-e', 'SMOKE_ALLOW_MUTATIONS=true', STACK,
        '/usr/local/bin/lab-tool', 'acceptance'], {timeout: 60});
      const accepted = JSON.parse(result.stdout);
      requireCondition(accepted.accepted === true &&
        JSON.stringify(accepted.statuses) === JSON.stringify([403, 204, 200, 204, 403]),
      'Original real native acceptance did not pass');
      writeFileSync(path.join(ROOT, 'acceptance-' + number + '.json'), result.stdout);
      passes.push(accepted);
    }
    const result = {passed: true, scope: 'fresh native candidate image; not browser acceptance',
      source_commit: SOURCE, packaging_commit: PACKAGING, image_id: image,
      binary_sha256: binaryHash, openfga_binary_sha256: fgaSha,
      default_failures: defaults, opt_in_model_timeout_seconds: 30,
      delayed_model_post_and_get_seconds: 6, actual_postgres_model_count: Number(models),
      empty_store_check: emptyStoreCheck, empty_store_check_deadline_failure: checkFailure,
      delayed_empty_store_check_seconds: 6, actual_postgres_tuple_count_before_app: Number(tuples),
      native_passes: passes, browser_acceptance_verified: false};
    saveJson('native-result.json', result);
    emit('native_proof', result);
  } finally {
    cleanupOwned(owned, runId);
  }
}

const args = process.argv.slice(2);
if (args.length === 1 && args[0] === 'proxy') serveProxy();
else if (args.length === 0) await main();
else throw new Error('Unexpected native proof mode');
