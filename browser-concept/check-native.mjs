// Real PostgreSQL + the retained OpenFGA ELF + the new Rust HTTP application.
// Synthetic credentials remain in a private temporary directory and are never reported.
import assert from 'node:assert/strict';
import {createHash, randomUUID} from 'node:crypto';
import {createWriteStream, promises as fs} from 'node:fs';
import {execFile, spawn} from 'node:child_process';
import {promisify} from 'node:util';
import {tmpdir} from 'node:os';
import path from 'node:path';

const execute = promisify(execFile);
const root = process.cwd();
const output = path.resolve('artifacts/browser-concept-native-proof');
const concept = path.resolve('artifacts/browser-concept-native/concept');
const openfga = path.resolve('source-candidate/artifacts/browser-openfga/openfga');
const base = 'http://127.0.0.1:13000';
const fga = 'http://127.0.0.1:18080';
const ownedName = `authz-concept-${process.env.GITHUB_RUN_ID ?? 'local'}-${randomUUID().slice(0,8)}`;
const record = async filename => {
  const bytes = await fs.readFile(filename);
  return {bytes:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')};
};
const run = (command,args,timeout=30000) => execute(command,args,{cwd:root,timeout,maxBuffer:2*1024*1024});
const delay = ms => new Promise(resolve=>setTimeout(resolve,ms));
await fs.mkdir(output,{recursive:true});
const privateDirectory = await fs.mkdtemp(path.join(tmpdir(),'authz-concept-'));
await fs.chmod(privateDirectory,0o700);
const credentialsPath = path.join(privateDirectory,'credentials.json');
const children=[];
let container;
let credentials;
let fgaChild;
let checksPassed=false;
const result={schemaVersion:1,kind:'real_browser_concept_native_proof',passed:false,
  startedAt:new Date().toISOString(),sourceCommit:process.env.GITHUB_SHA ??
    (await run('git',['rev-parse','HEAD'])).stdout.trim(),
  runId:Number(process.env.GITHUB_RUN_ID ?? 0),runAttempt:Number(process.env.GITHUB_RUN_ATTEMPT ?? 1),
  conceptBinary:await record(concept),model:await record('browser-concept/model.json'),
  openfga:await record(openfga),openfgaWarmupModel:await record('fga/model.json'),
  operations:[],cycles:[],cleanupErrors:[]};

function launch(label,executable,args,env) {
  const log=createWriteStream(path.join(output,`${label}.log`),{flags:'wx'});
  const child=spawn(executable,args,{cwd:root,env:{...process.env,...env},stdio:['ignore','pipe','pipe']});
  child.stdout.pipe(log,{end:false});child.stderr.pipe(log,{end:false});
  const state={label,child,log,spawnError:undefined,closed:false};
  state.done=new Promise(resolve=>{
    child.once('error',error=>{state.spawnError=error;});
    child.once('close',(code,signal)=>{state.closed=true;state.outcome={code,signal};log.end(resolve);});
  });
  children.push(state);
  return state;
}
async function stop(state) {
  if (!state || state.closed) return;
  state.child.kill('SIGTERM');
  await Promise.race([state.done,delay(5000)]);
  if(!state.closed){state.child.kill('SIGKILL');await Promise.race([state.done,delay(5000)]);}
  assert(state.closed,`${state.label} did not stop`);
}
async function waitUntil(check,seconds) {
  const deadline=Date.now()+seconds*1000;
  while(Date.now()<deadline){
    for(const state of children)assert(!state.spawnError&&!state.closed,`${state.label} stopped during native setup`);
    if(await check())return;
    await delay(500);
  }
  throw new Error('Native service readiness deadline expired');
}
async function decoded(response) {
  const bytes=new Uint8Array(await response.arrayBuffer());
  assert(bytes.length<=24*1024,'Native HTTP response exceeded its declared bound');
  if(response.status===204){assert.equal(bytes.length,0);return null;}
  assert.match(response.headers.get('content-type')??'',/^application\/json(?:;|$)/i);
  return JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(bytes));
}
async function operation(actor,operation,expectedStatus,extra={}) {
  const routes={permissions:['GET','/concept/permissions'],read:['GET','/concept/document'],
    grant:['PUT','/concept/document/viewer'],revoke:['DELETE','/concept/document/viewer']};
  const [method,url]=routes[operation];
  const headers={};
  if(actor)headers.authorization=`Bearer ${actor==='invalid'?'a'.repeat(64):credentials[actor]}`;
  const input=method==='PUT'||method==='DELETE'?(extra.input??{user:'bob'}):undefined;
  if(input)headers['content-type']='application/json';
  const started=performance.now();
  const response=await fetch(base+url,{method,headers,redirect:'error',signal:AbortSignal.timeout(5000),
    ...(input?{body:JSON.stringify(input)}:{})});
  const body=await decoded(response);
  const observed={actor:actor??'anonymous',operation,method,path:url,status:response.status,
    body,elapsedMs:Math.round(performance.now()-started)};
  result.operations.push(observed);
  assert.equal(response.status,expectedStatus,`${actor??'anonymous'} ${operation} returned ${response.status}`);
  return observed;
}
async function tuples(store) {
  const response=await fetch(`${fga}/stores/${store}/read`,{method:'POST',headers:{'content-type':'application/json'},
    body:JSON.stringify({page_size:100,consistency:'HIGHER_CONSISTENCY'}),signal:AbortSignal.timeout(5000)});
  assert.equal(response.status,200);
  const body=await decoded(response);
  assert.equal(body.continuation_token??'','');
  return (body.tuples??[]).map(tuple=>tuple.key);
}

async function bridgeProbe(health) {
  const child=spawn(concept,['bridge'],{cwd:root,env:{...process.env,
    CONCEPT_BASE_URL:base,CONCEPT_CREDENTIALS_FILE:credentialsPath},stdio:['pipe','pipe','pipe']});
  const stdout=[],stderr=[];
  let bytes=0;
  for(const [stream,parts] of [[child.stdout,stdout],[child.stderr,stderr]])stream.on('data',chunk=>{
    bytes+=chunk.length;
    if(bytes>128*1024)child.kill('SIGKILL');
    else parts.push(chunk);
  });
  const timer=setTimeout(()=>child.kill('SIGKILL'),15000);
  const done=new Promise((resolve,reject)=>{child.once('error',reject);child.once('close',(code,signal)=>resolve({code,signal}));});
  const commands=[{id:1,operation:'read',actor:'bob'},{id:2,operation:'permissions',actor:'alice'}];
  child.stdin.on('error',()=>{});
  child.stdin.end(commands.map(command=>'CONCEPT|command|'+JSON.stringify(command)+'\n').join(''));
  let outcome;
  try{outcome=await done;}finally{clearTimeout(timer);}
  assert(bytes<=128*1024,'Native bridge output exceeded its bound');
  await fs.writeFile(path.join(output,'bridge.log'),Buffer.concat([...stdout,...stderr]));
  assert.deepEqual(outcome,{code:0,signal:null},'Native bridge did not close normally');
  const lines=Buffer.concat(stdout).toString('utf8').trim().split(/\r?\n/);
  assert.equal(lines.length,3,'Unexpected native bridge protocol output');
  assert(lines[0].startsWith('CONCEPT|ready|'));
  assert.deepEqual(JSON.parse(lines[0].slice('CONCEPT|ready|'.length)),health);
  const observations=lines.slice(1).map(line=>{
    assert(line.startsWith('CONCEPT|response|'));
    return JSON.parse(line.slice('CONCEPT|response|'.length));
  });
  for(let i=0;i<2;i++){
    const actual=observations[i];
    for(const key of ['id','actor','operation'])assert.equal(actual[key],commands[i][key]);
    assert.equal(actual.error,undefined);
    assert(Number.isSafeInteger(actual.elapsed_ms)&&actual.elapsed_ms>=0);
  }
  assert.equal(observations[0].http_status,403);
  assert.deepEqual(observations[0].body,{error:'forbidden'});
  assert.equal(observations[1].http_status,200);
  assert.deepEqual(observations[1].body,{actor:'alice',document:'demo',can_read:true,can_share:true});
  return {passed:true,health,observations};
}

try {
  const nativeBuild=JSON.parse(await fs.readFile('artifacts/browser-concept-native/build.json','utf8'));
  assert.equal(nativeBuild.sourceCommit,result.sourceCommit);
  assert.deepEqual(nativeBuild.binary,result.conceptBinary);
  assert.deepEqual(nativeBuild.model,result.model);
  assert.equal(result.openfga.sha256,'d779834ae225c95502b957efc96a6153937bb2f5f2a1cef9f134847ae8c0d02b');
  assert.deepEqual(result.openfgaWarmupModel,{bytes:52709,
    sha256:'a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d'});
  const created=await run('docker',['run','--detach','--name',ownedName,'--label',`io.authzlab.concept-owner=${ownedName}`,
    '--shm-size=128m','-e','POSTGRES_HOST_AUTH_METHOD=trust','-e','POSTGRES_USER=authzlab','-e','POSTGRES_DB=concept',
    '-p','127.0.0.1:25432:5432','postgres:17-bookworm'],120000);
  container=created.stdout.trim();assert.match(container,/^[0-9a-f]{64}$/);
  await waitUntil(async()=>{
    try{await run('docker',['exec',container,'pg_isready','-U','authzlab','-d','concept'],5000);return true;}
    catch{return false;}
  },60);
  const imageId=(await run('docker',['inspect','--format','{{.Image}}',container])).stdout.trim();
  const version=(await run('docker',['exec',container,'psql','-U','authzlab','-d','concept','-Atc','SELECT version()'])).stdout.trim();
  assert.match(version,/^PostgreSQL 17\./);
  result.postgres={image:'postgres:17-bookworm',imageId,version};
  fgaChild=launch('openfga',openfga,['run'],{
    FGA_MODEL:path.resolve('fga/model.json'),
    OPENFGA_DATASTORE_ENGINE:'memory',OPENFGA_HTTP_ADDR:'127.0.0.1:18080',OPENFGA_GRPC_ADDR:'127.0.0.1:18081',
    OPENFGA_PLAYGROUND_ENABLED:'false',OPENFGA_METRICS_ENABLED:'false',OPENFGA_LOG_LEVEL:'warn',OPENFGA_REQUEST_TIMEOUT:'3s'});
  await waitUntil(async()=>{
    try{const r=await fetch(fga+'/healthz',{signal:AbortSignal.timeout(2000)});return r.status===200&&(await r.json()).status==='SERVING';}
    catch{return false;}
  },90);
  launch('concept',concept,['serve'],{DATABASE_URL:'postgres://authzlab@127.0.0.1:25432/concept?sslmode=disable',
    FGA_URL:fga,CONCEPT_LISTEN_ADDR:'127.0.0.1:13000',CONCEPT_CREDENTIALS_FILE:credentialsPath});
  let health;
  await waitUntil(async()=>{
    try{const r=await fetch(base+'/concept/health',{signal:AbortSignal.timeout(3000)});if(r.status!==200)return false;health=await decoded(r);return true;}
    catch{return false;}
  },90);
  assert.equal(health.protocol,1);assert.equal(health.document,'demo');assert.equal(health.owner,'alice');
  assert.equal(health.model_sha256,result.model.sha256);
  assert.match(health.store_id,/^[0-9A-HJKMNP-TV-Z]{26}$/);assert.match(health.model_id,/^[0-9A-HJKMNP-TV-Z]{26}$/);
  result.health=health;
  assert.equal((await fs.stat(credentialsPath)).mode&0o777,0o600);
  try{credentials=JSON.parse(await fs.readFile(credentialsPath,'utf8'));}
  catch{throw new Error('The private native credentials could not be read');}
  assert.deepEqual(Object.keys(credentials).sort(),['alice','bob']);
  for(const token of Object.values(credentials))assert(typeof token==='string'&&/^[0-9a-f]{64}$/.test(token),'Invalid private credential format');
  assert(credentials.alice!==credentials.bob,'Demo identities must have different credentials');
  // The retained protobuf gateway emits the absent condition as explicit null.
  const owner={user:'user:alice',relation:'owner',object:'document:demo',condition:null};
  assert.deepEqual(await tuples(health.store_id),[owner]);
  result.initialOwnerReadback=true;
  const counts=(await run('docker',['exec',container,'psql','-U','authzlab','-d','concept','-Atc',
    'SELECT (SELECT count(*) FROM browser_concept.documents),(SELECT count(*) FROM browser_concept.sessions)'])).stdout.trim();
  assert.equal(counts,'1|2');result.sqlFixture={documents:1,sessions:2};
  await operation(null,'read',401);await operation('invalid','read',401);
  assert.deepEqual((await operation('alice','permissions',200)).body,{actor:'alice',document:'demo',can_read:true,can_share:true});
  assert.deepEqual((await operation('bob','permissions',200)).body,{actor:'bob',document:'demo',can_read:false,can_share:false});
  result.bridge=await bridgeProbe(health);
  const fixture=JSON.parse(await fs.readFile('browser-concept/fixtures.json','utf8'));
  assert.deepEqual((await operation('alice','read',200)).body,fixture.document);
  await operation('bob','grant',403);await operation('bob','revoke',403);
  await operation('alice','grant',400,{input:{user:'alice'}});
  await operation('alice','grant',400,{input:{user:'bob',actor:'alice'}});
  for(let cycle=1;cycle<=2;cycle++){
    const sequence=[];
    sequence.push((await operation('bob','read',403)).status);
    sequence.push((await operation('alice','grant',204)).status);
    const allowed=await operation('bob','read',200);sequence.push(allowed.status);assert.deepEqual(allowed.body,fixture.document);
    assert.deepEqual((await operation('bob','permissions',200)).body,{actor:'bob',document:'demo',can_read:true,can_share:false});
    await operation('bob','grant',403);await operation('bob','revoke',403);
    sequence.push((await operation('alice','revoke',204)).status);
    sequence.push((await operation('bob','read',403)).status);
    assert.deepEqual((await operation('bob','permissions',200)).body,{actor:'bob',document:'demo',can_read:false,can_share:false});
    assert.deepEqual(sequence,[403,204,200,204,403]);
    assert.deepEqual(await tuples(health.store_id),[owner]);
    result.cycles.push({id:cycle,statuses:sequence,ownerPreserved:true,viewerRemoved:true});
  }
  await stop(fgaChild);
  await operation('alice','read',503);await operation('alice','grant',503);
  result.upstreamFailureDenied=true;
  checksPassed=true;
} catch(error) {
  result.failure=String(error);
  console.error('Native concept proof failed:',String(error));
} finally {
  for(const child of [...children].reverse()){
    try{await stop(child);}catch(error){result.cleanupErrors.push(`${child.label}: ${error}`);}
  }
  if(container){
    try{
      const owner=(await run('docker',['inspect','--format','{{index .Config.Labels "io.authzlab.concept-owner"}}',container])).stdout.trim();
      assert.equal(owner,ownedName);
      const logs=await run('docker',['logs',container]);
      await fs.writeFile(path.join(output,'postgres.log'),logs.stdout+logs.stderr);
      await run('docker',['rm','-f','-v',container]);
    }catch(error){result.cleanupErrors.push(`postgres: ${error}`);}
  }
  await fs.rm(privateDirectory,{recursive:true,force:true});
  result.serviceOutcomes=children.map(({label,outcome})=>({label,...outcome}));
  result.passed=checksPassed&&result.cleanupErrors.length===0;
  result.finishedAt=new Date().toISOString();
  await fs.writeFile(path.join(output,'result.json'),JSON.stringify(result,null,2)+'\n');
  console.log(JSON.stringify({passed:result.passed,cycles:result.cycles,upstreamFailureDenied:result.upstreamFailureDenied,
    conceptBinary:result.conceptBinary,model:result.model}));
  if(!result.passed)process.exitCode=1;
}
