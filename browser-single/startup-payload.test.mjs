import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import test from 'node:test';
import {loadStartupPayload} from './pages/startup-payload.mjs';

const base = 'http://localhost/wasm/launcher.js';
const digest = body => createHash('sha256').update(body).digest('hex');
function fixture() {
  const files = {
    'guest.sh':'guest fixture', 'entrypoint.sh':'entrypoint fixture',
    'runtime/openfga':'binary fixture', 'runtime/bootstrap':'bootstrap fixture'
  };
  const manifest = {
    base_source_commit:'99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58',
    openfga_build:{fixture:true},
    files:Object.fromEntries(Object.entries(files).map(([name,body])=>[name,{
      bytes:Buffer.byteLength(body),sha256:digest(body)
    }]))
  };
  manifest.bootstrap_build = {
    schema_version:1,source_commit:'64d6fb4614b6674284a32cee2f4f84179dfb2a2f',
    target:'linux/amd64',image_id:'sha256:'+'a'.repeat(64),
    binary_bytes:manifest.files['runtime/bootstrap'].bytes,
    binary_sha256:manifest.files['runtime/bootstrap'].sha256,
    dockerfile_sha256:digest('Dockerfile fixture'),cargo_lock_sha256:digest('Cargo.lock fixture'),
    bootstrap_source_sha256:digest('bootstrap.rs fixture'),
    bootstrap_tests_sha256:digest('bootstrap_tests.rs fixture'),
    timeout_scope:'bootstrap_model_post_and_get',default_model_timeout_seconds:5,
    min_model_timeout_seconds:5,max_model_timeout_seconds:30,browser_model_timeout_seconds:30,
    empty_store_check:'one_real_higher_consistency_ownership_deny',empty_store_check_default:false,
    browser_empty_store_check:true,empty_store_check_timeout_seconds:5,empty_store_check_tests:'passed',
    native_tests:'passed',native_test_log_sha256:digest('native test log fixture')
  };
  const requests = [];
  return {files,manifest,requests,fetcher:async url=> {
    const name = url.pathname.slice('/wasm/'.length);
    requests.push(name);
    if (name === 'runtime/startup-manifest.json') return Response.json(manifest);
    return new Response(files[name] ?? '',{status:files[name] ? 200 : 404});
  }};
}

test('load exactly four verified startup files and retain bootstrap build provenance', async () => {
  const input = fixture();
  const {manifest,payload} = await loadStartupPayload(base,input.fetcher);
  assert.deepEqual(manifest,input.manifest);
  assert.deepEqual(payload.map(file=>file.name),['guest.sh','entrypoint.sh','openfga','bootstrap']);
  assert.equal(new TextDecoder().decode(payload[2].bytes),'binary fixture');
  assert.equal(new TextDecoder().decode(payload[3].bytes),'bootstrap fixture');
  assert.equal(manifest.bootstrap_build.binary_sha256,digest(payload[3].bytes));
});

test('reject corrupt or truncated OpenFGA and bootstrap binaries before returning a payload', async () => {
  for (const file of ['runtime/openfga','runtime/bootstrap']) {
    const corrupt = fixture();
    corrupt.files[file] = 'X'+corrupt.files[file].slice(1);
    await assert.rejects(loadStartupPayload(base,corrupt.fetcher),/checksum mismatch/);
    const truncated = fixture();
    truncated.files[file] = truncated.files[file].slice(0,-1);
    await assert.rejects(loadStartupPayload(base,truncated.fetcher),/size mismatch/);
  }
});

test('reject a missing fourth payload and unknown or invalid manifest entries before file requests', async () => {
  for (const modify of [
    input=>delete input.manifest.files['guest.sh'],
    input=>delete input.manifest.files['runtime/bootstrap'],
    input=>{input.manifest.files['../other'] = input.manifest.files['guest.sh'];},
    input=>{input.manifest.files['runtime/openfga'].sha256 = 'not a checksum';},
    input=>{input.manifest.files['runtime/bootstrap'].bytes = 0;}
  ]) {
    const input = fixture();
    modify(input);
    await assert.rejects(loadStartupPayload(base,input.fetcher),/manifest|checksum/);
    assert.deepEqual(input.requests,['runtime/startup-manifest.json']);
  }
});

test('require reviewed native bootstrap provenance before downloading executable files', async () => {
  for (const modify of [
    input=>delete input.manifest.bootstrap_build,
    input=>{input.manifest.bootstrap_build = null;},
    input=>{input.manifest.bootstrap_build = [];},
    input=>{input.manifest.bootstrap_build.source_commit = 'b'.repeat(40);},
    input=>{input.manifest.bootstrap_build.schema_version = 2;},
    input=>{input.manifest.bootstrap_build.target = 'linux/arm64';},
    input=>{input.manifest.bootstrap_build.image_id = 'mutable:tag';},
    input=>{input.manifest.bootstrap_build.timeout_scope = 'all_requests';},
    input=>{input.manifest.bootstrap_build.default_model_timeout_seconds = 30;},
    input=>{input.manifest.bootstrap_build.min_model_timeout_seconds = 1;},
    input=>{input.manifest.bootstrap_build.max_model_timeout_seconds = 60;},
    input=>{input.manifest.bootstrap_build.browser_model_timeout_seconds = 31;},
    input=>{input.manifest.bootstrap_build.native_tests = 'not-run';},
    input=>{input.manifest.bootstrap_build.native_test_log_sha256 = '';},
    input=>{input.manifest.bootstrap_build.bootstrap_source_sha256 = '';},
    input=>{input.manifest.bootstrap_build.bootstrap_tests_sha256 = '';},
    input=>{input.manifest.bootstrap_build.dockerfile_sha256 = '';},
    input=>{input.manifest.bootstrap_build.cargo_lock_sha256 = '';}
  ]) {
    const input = fixture();
    modify(input);
    await assert.rejects(loadStartupPayload(base,input.fetcher),/bootstrap build provenance/);
    assert.deepEqual(input.requests,['runtime/startup-manifest.json']);
  }
});


test('require the opt-in real empty-store deny proof without broadening its ordinary deadline', async () => {
  const missing = ['empty_store_check','empty_store_check_default','browser_empty_store_check',
    'empty_store_check_timeout_seconds','empty_store_check_tests'];
  for (const key of missing) {
    const input = fixture();
    delete input.manifest.bootstrap_build[key];
    await assert.rejects(loadStartupPayload(base,input.fetcher),/bootstrap build provenance/);
    assert.deepEqual(input.requests,['runtime/startup-manifest.json']);
  }
  for (const [key,value] of [
    ['source_commit','53ff85cfdd3d84c26ae432189790e1e0f72d61db'],
    ['empty_store_check','synthetic_deny'],
    ['empty_store_check_default',true],['browser_empty_store_check',false],
    ['empty_store_check_timeout_seconds',30],['empty_store_check_tests','not-run']
  ]) {
    const input = fixture();
    input.manifest.bootstrap_build[key] = value;
    await assert.rejects(loadStartupPayload(base,input.fetcher),/bootstrap build provenance/);
    assert.deepEqual(input.requests,['runtime/startup-manifest.json']);
  }
});

test('require the metadata and downloaded bootstrap entry to identify the same binary', async () => {
  for (const modify of [
    input=>{input.manifest.bootstrap_build.binary_bytes++;},
    input=>{input.manifest.bootstrap_build.binary_sha256 = 'b'.repeat(64);},
    input=>{input.manifest.files['runtime/bootstrap'].sha256 = 'c'.repeat(64);}
  ]) {
    const input = fixture();
    modify(input);
    await assert.rejects(loadStartupPayload(base,input.fetcher),/bootstrap build provenance/);
    assert.deepEqual(input.requests,['runtime/startup-manifest.json']);
  }
});

test('missing manifest or either startup binary fails closed', async () => {
  await assert.rejects(loadStartupPayload(base,async()=>new Response('',{status:404})),/Cannot load startup manifest/);
  for (const file of ['runtime/openfga','runtime/bootstrap']) {
    const input = fixture();
    delete input.files[file];
    await assert.rejects(loadStartupPayload(base,input.fetcher),/Cannot load runtime\/(openfga|bootstrap)/);
  }
});

test('a pending fourth binary prevents startup payload completion', async () => {
  const input = fixture();
  let release;
  const held = new Promise(resolve=>{release=resolve;});
  let entered;
  const requested = new Promise(resolve=>{entered=resolve;});
  let completed = false;
  const loading = loadStartupPayload(base,async url=>{
    if (url.pathname.endsWith('/runtime/bootstrap')) {
      entered();
      await held;
    }
    return input.fetcher(url);
  }).then(value=>{completed=true;return value;});
  await requested;
  await Promise.resolve();
  assert.equal(completed,false);
  release();
  assert.equal((await loading).payload.length,4);
});
