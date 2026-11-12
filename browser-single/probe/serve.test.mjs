import assert from 'node:assert/strict';
import {spawn, spawnSync} from 'node:child_process';
import {once} from 'node:events';
import {mkdtemp, mkdir, readFile, rm, symlink, writeFile} from 'node:fs/promises';
import {request} from 'node:http';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import test from 'node:test';

const serverScript = fileURLToPath(new URL('./serve.mjs', import.meta.url));
const runnerScript = fileURLToPath(new URL('./run.mjs', import.meta.url));
const filenames = ['load.js', 'arg-module.js', 'out.js', 'qemu-system-x86_64.data', 'qemu-system-x86_64.wasm'];

async function fixture(t) {
  const root = await mkdtemp(path.join(tmpdir(), 'browser-probe-test-'));
  t.after(() => rm(root, {recursive: true, force: true}));
  const artifact = path.join(root, 'artifact');
  await mkdir(artifact);
  for (const name of filenames) await writeFile(path.join(artifact, name), `fixture:${name}`);
  await writeFile(path.join(root, 'outside.txt'), 'outside the serving directory');
  return {root, artifact};
}

async function start(t, artifact) {
  const child = spawn(process.execPath, [serverScript, '--artifact', artifact, '--port', '0'], {stdio: ['ignore', 'pipe', 'pipe']});
  t.after(async () => {
    if (child.exitCode !== null || child.signalCode !== null) return;
    const closed = once(child, 'close');
    child.kill();
    await closed;
  });
  return new Promise((resolve, reject) => {
    let output = '', errors = '';
    const timer = setTimeout(() => reject(new Error('Server did not become ready.')), 5000);
    child.on('error', error => { clearTimeout(timer); reject(error); });
    child.stderr.on('data', data => { errors += data; });
    child.once('close', code => { clearTimeout(timer); reject(new Error(`Server exited ${code}: ${errors}`)); });
    child.stdout.on('data', data => {
      output += data;
      const match = output.match(/http:\/\/127\.0\.0\.1:\d+\//);
      if (match) { clearTimeout(timer); resolve(new URL(match[0])); }
    });
  });
}

function http(url, requestPath, method = 'GET') {
  return new Promise((resolve, reject) => {
    const req = request({hostname: url.hostname, port: url.port, path: requestPath, method}, res => {
      const chunks = [];
      res.on('data', data => chunks.push(data));
      res.on('error', reject);
      res.on('end', () => resolve({status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks)}));
    });
    req.on('error', reject);
    req.setTimeout(5000, () => req.destroy(new Error('HTTP request timed out.')));
    req.end();
  });
}

function assertIsolation(headers) {
  assert.equal(headers['cross-origin-opener-policy'], 'same-origin');
  assert.equal(headers['cross-origin-embedder-policy'], 'require-corp');
  assert.equal(headers['cross-origin-resource-policy'], 'same-origin');
  assert.equal(headers['x-content-type-options'], 'nosniff');
}

test('loopback server serves only probe and artifact files with isolation headers', async t => {
  const {root, artifact} = await fixture(t);
  const url = await start(t, artifact);
  for (const requestPath of ['/', '/index.html?boot=cold&memory=512M', '/probe.js']) {
    const response = await http(url, requestPath);
    assert.equal(response.status, 200);
    assertIsolation(response.headers);
    assert.match(response.headers['content-type'], requestPath === '/probe.js' ? /^text\/javascript/ : /^text\/html/);
    assert.ok(response.body.length > 0);
  }
  for (const name of filenames) {
    const response = await http(url, `/artifact/${name}`);
    assert.equal(response.status, 200);
    assertIsolation(response.headers);
    assert.equal(response.body.toString(), `fixture:${name}`);
    const type = name.endsWith('.wasm') ? 'application/wasm' : name.endsWith('.js') ? 'text/javascript' : 'application/octet-stream';
    assert.equal(response.headers['content-type'], type);
    const head = await http(url, `/artifact/${name}`, 'HEAD');
    assert.equal(head.status, 200);
    assert.equal(head.headers['content-length'], String(response.body.length));
    assert.equal(head.headers['content-type'], type);
    assertIsolation(head.headers);
    assert.equal(head.body.length, 0);
  }
  for (const requestPath of ['/artifact/', '/README.md', '/serve.mjs', '/artifact/missing', '/artifact/../outside.txt', '/artifact/%2e%2e%2foutside.txt']) {
    const response = await http(url, requestPath);
    assert.equal(response.status, 404, requestPath);
    assertIsolation(response.headers);
    assert.doesNotMatch(response.body.toString(), /outside the serving directory/);
  }
  assert.equal((await http(url, '/artifact/%zz')).status, 400);
  const post = await http(url, '/', 'POST');
  assert.equal(post.status, 405);
  assert.equal(post.headers.allow, 'GET, HEAD');
  assertIsolation(post.headers);
  await rm(path.join(artifact, 'load.js'));
  await symlink(path.join(root, 'outside.txt'), path.join(artifact, 'load.js'));
  assert.equal((await http(url, '/artifact/load.js')).status, 404);
});

test('server fails before readiness for missing or escaped artifacts and invalid ports', async t => {
  const {root, artifact} = await fixture(t);
  const launch = extra => spawnSync(process.execPath, [serverScript, '--artifact', artifact, ...extra], {encoding: 'utf8', timeout: 5000});
  const invalidPort = launch(['--port', '65536']);
  assert.equal(invalidPort.status, 1);
  assert.match(invalidPort.stderr, /--port/);
  await rm(path.join(artifact, 'qemu-system-x86_64.wasm'));
  const missing = launch([]);
  assert.equal(missing.status, 1);
  assert.equal(missing.stdout, '');
  await symlink(path.join(root, 'outside.txt'), path.join(artifact, 'qemu-system-x86_64.wasm'));
  const escaped = launch([]);
  assert.equal(escaped.status, 1);
  assert.match(escaped.stderr, /escapes/);
  assert.equal(escaped.stdout, '');
});

test('runner records launch failures and stops its owned Node.js server', async t => {
  const {root, artifact} = await fixture(t);
  const out = path.join(root, 'results');
  const result = spawnSync(process.execPath, [runnerScript, '--artifact', artifact, '--out', out, '--chromium', path.join(root, 'missing-browser')], {encoding: 'utf8', timeout: 15000});
  assert.equal(result.status, 2, result.stderr || result.stdout);
  const report = JSON.parse(await readFile(path.join(out, 'result.json'), 'utf8'));
  assert.equal(report.phase, 'browser-launch');
  assert.equal(report.outcome, 'infrastructure-failure');
  assert.equal(report.authorizationVerified, false);
  assert.equal(report.httpBridgeImplemented, false);
  assert.ok(report.events.some(event => event.kind === 'server.ready'));
  assert.ok(report.events.some(event => event.kind === 'server.exit'));
  await assert.rejects(http(new URL(report.url), '/'), {code: 'ECONNREFUSED'});
});
