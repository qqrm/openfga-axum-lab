import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import test from 'node:test';
import {loadStartupPayload} from './pages/startup-payload.mjs';

const base = 'http://localhost/wasm/launcher.js';
function fixture() {
  const files = {'guest.sh':'guest fixture','entrypoint.sh':'entrypoint fixture','runtime/openfga':'binary fixture'};
  const manifest = {
    base_source_commit:'99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58',
    openfga_build:{fixture:true},
    files:Object.fromEntries(Object.entries(files).map(([name,body])=>[name,{
      bytes:Buffer.byteLength(body),sha256:createHash('sha256').update(body).digest('hex')
    }]))
  };
  return {files,manifest,fetcher:async url=> {
    const name = url.pathname.slice('/wasm/'.length);
    if (name === 'runtime/startup-manifest.json') return Response.json(manifest);
    return new Response(files[name] ?? '',{status:files[name] ? 200 : 404});
  }};
}

test('load only the three verified startup files and retain their build provenance', async () => {
  const input = fixture();
  const {manifest,payload} = await loadStartupPayload(base,input.fetcher);
  assert.deepEqual(manifest,input.manifest);
  assert.deepEqual(payload.map(file=>file.name),['guest.sh','entrypoint.sh','openfga']);
  assert.equal(new TextDecoder().decode(payload[2].bytes),'binary fixture');
});

test('reject corrupted or truncated startup binaries before VM initialization', async () => {
  const corrupt = fixture();
  corrupt.files['runtime/openfga'] = 'Binary fixture';
  await assert.rejects(loadStartupPayload(base,corrupt.fetcher),/checksum mismatch/);
  const truncated = fixture();
  truncated.files['runtime/openfga'] = 'binary';
  await assert.rejects(loadStartupPayload(base,truncated.fetcher),/size mismatch/);
});

test('reject missing, unexpected or invalid manifest entries', async () => {
  for (const modify of [
    input=>delete input.manifest.files['guest.sh'],
    input=>{input.manifest.files['../other'] = input.manifest.files['guest.sh'];},
    input=>{input.manifest.files['runtime/openfga'].sha256 = 'not a checksum';},
    input=>{input.manifest.files['runtime/openfga'].bytes = 0;}
  ]) {
    const input = fixture();
    modify(input);
    await assert.rejects(loadStartupPayload(base,input.fetcher),/manifest|checksum/);
  }
});

test('missing manifest or startup file fails closed', async () => {
  await assert.rejects(loadStartupPayload(base,async()=>new Response('',{status:404})),/Cannot load startup manifest/);
  const input = fixture();
  delete input.files['runtime/openfga'];
  await assert.rejects(loadStartupPayload(base,input.fetcher),/Cannot load runtime\/openfga/);
});
