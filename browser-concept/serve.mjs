// Serve an already assembled concept bundle on this computer. The backend runs
// in the browser VM; this process only reads static files below the bundle root.
import assert from 'node:assert/strict';
import {createReadStream, promises as fs} from 'node:fs';
import {createServer} from 'node:http';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

assert(process.argv.length <= 3, 'Usage: node serve.mjs [assembled-concept-directory]');
const root = await fs.realpath(path.resolve(process.argv[2] ?? fileURLToPath(new URL('.', import.meta.url))));
assert((await fs.stat(path.join(root, 'index.html'))).isFile(), 'Choose the unpacked concept site, not its source directory');
assert((await fs.stat(path.join(root, 'runtime/startup-manifest.json'))).isFile(), 'The assembled runtime manifest is missing');
const port = Number(process.env.PORT ?? 8765);
assert(Number.isSafeInteger(port) && port >= 1024 && port <= 65535, 'PORT must be an integer from 1024 to 65535');
const types = {'.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript',
  '.css': 'text/css', '.wasm': 'application/wasm', '.json': 'application/json', '.sh': 'text/plain'};
const server = createServer(async (request, response) => {
  try {
    if (!['GET', 'HEAD'].includes(request.method)) {
      response.writeHead(405, {Allow: 'GET, HEAD'}); response.end(); return;
    }
    const url = new URL(request.url, 'http://localhost');
    const name = decodeURIComponent(url.pathname) + (url.pathname.endsWith('/') ? 'index.html' : '');
    const candidate = path.resolve(root, '.' + name);
    assert(candidate.startsWith(root + path.sep));
    const file = await fs.realpath(candidate);
    assert(file.startsWith(root + path.sep));
    const stat = await fs.stat(file);
    assert(stat.isFile());
    // The site's own scoped service worker establishes browser isolation.
    response.writeHead(200, {'Content-Type': types[path.extname(file)] ?? 'application/octet-stream',
      'Content-Length': stat.size, 'Cache-Control': 'no-cache'});
    if (request.method === 'HEAD') response.end();
    else createReadStream(file).on('error', error => response.destroy(error)).pipe(response);
  } catch { response.writeHead(404); response.end(); }
});
server.on('error', error => { console.error('Cannot serve the concept bundle: ' + error.message); process.exitCode = 1; });
server.listen(port, '127.0.0.1', () => {
  console.log(`Open http://127.0.0.1:${port}/ and click Start the playground.`);
  console.log('This server serves static files only. Press Ctrl+C to stop.');
});
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => {
  server.closeAllConnections(); server.close();
});
