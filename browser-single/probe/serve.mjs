#!/usr/bin/env node
// Serve the supplied container2wasm artifact on loopback without a backend.
import {createReadStream} from 'node:fs';
import {realpath, stat} from 'node:fs/promises';
import {createServer} from 'node:http';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {parseArgs} from 'node:util';

const artifactFiles = ['load.js', 'arg-module.js', 'out.js', 'qemu-system-x86_64.data', 'qemu-system-x86_64.wasm'];
const contentTypes = {'.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.wasm': 'application/wasm'};
const isolationHeaders = {
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Cross-Origin-Embedder-Policy': 'require-corp',
  'Cross-Origin-Resource-Policy': 'same-origin',
  'X-Content-Type-Options': 'nosniff',
};

async function checkedFile(root, filename) {
  const file = await realpath(path.join(root, filename));
  const relative = path.relative(root, file);
  if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error(`File escapes its serving directory: ${filename}`);
  }
  const info = await stat(file);
  if (!info.isFile()) throw new Error(`Not a regular file: ${filename}`);
  return {file, size: info.size};
}

async function main() {
  const {values} = parseArgs({options: {artifact: {type: 'string'}, port: {type: 'string', default: '8765'}}});
  if (!values.artifact) throw new Error('Usage: node serve.mjs --artifact DIRECTORY [--port PORT]');
  if (!/^\d+$/.test(values.port) || Number(values.port) > 65535) throw new Error('--port must be an integer from 0 to 65535.');
  const artifact = await realpath(path.resolve(values.artifact));
  const probe = await realpath(fileURLToPath(new URL('.', import.meta.url)));
  const routes = new Map([
    ['/', [probe, 'index.html']],
    ['/index.html', [probe, 'index.html']],
    ['/probe.js', [probe, 'probe.js']],
  ]);
  for (const filename of artifactFiles) {
    await checkedFile(artifact, filename);
    routes.set(`/artifact/${filename}`, [artifact, filename]);
  }
  const server = createServer(async (request, response) => {
    for (const [name, value] of Object.entries(isolationHeaders)) response.setHeader(name, value);
    const reject = (status, message) => {
      response.writeHead(status, {'Content-Type': 'text/plain; charset=utf-8'});
      response.end(message);
    };
    if (request.method !== 'GET' && request.method !== 'HEAD') {
      response.setHeader('Allow', 'GET, HEAD');
      reject(405, 'Method not allowed.\n');
      return;
    }
    let pathname;
    try { pathname = decodeURIComponent(request.url.split('?')[0]); }
    catch { reject(400, 'Invalid URL encoding.\n'); return; }
    const route = routes.get(pathname);
    if (!route) { reject(404, 'Not found.\n'); return; }
    let asset;
    try { asset = await checkedFile(...route); }
    catch { reject(404, 'Not found.\n'); return; }
    response.setHeader('Content-Type', contentTypes[path.extname(route[1])] || 'application/octet-stream');
    response.setHeader('Content-Length', asset.size);
    if (request.method === 'HEAD') { response.end(); return; }
    const stream = createReadStream(asset.file);
    stream.on('error', () => response.destroy());
    response.on('close', () => stream.destroy());
    stream.pipe(response);
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(Number(values.port), '127.0.0.1', resolve);
  });
  console.log(`http://127.0.0.1:${server.address().port}/`);
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
