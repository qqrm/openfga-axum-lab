// Invoke the exact production browser checker with one allowlisted engine.
// All assertions, service inputs and per-wait deadlines remain unchanged.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {spawn,spawnSync} from 'node:child_process';

const sourceCommit = '427ef0305c48e094c2873cd3ce380f7f0e3d5683';
const engine = process.env.BROWSER_DIAGNOSTIC_ENGINE;
assert(['chromium','firefox'].includes(engine),'Expected one allowlisted engine');
const source = path.resolve('source-candidate');
const evidenceDirectory = path.resolve('artifacts/bootstrap-browser-gate');
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const materialization = JSON.parse(await fs.readFile(path.join(evidenceDirectory,'materialization.json'),'utf8'));
assert.equal(materialization.sourceCommit,sourceCommit);
const head = spawnSync('git',['rev-parse','HEAD'],{cwd:source,encoding:'utf8'});
assert.equal(head.status,0);
assert.equal(head.stdout.trim(),sourceCommit);
const originalPath = path.join(source,'browser-single/check-pages.mjs');
const original = await fs.readFile(originalPath,'utf8');
const initializer = "for (const [name, engine] of [['chromium',chromium],['firefox',firefox]]) {";
const singleton = engine==='chromium'
  ? "for (const [name, engine] of [['chromium',chromium]]) {"
  : "for (const [name, engine] of [['firefox',firefox]]) {";
assert.equal(original.split(initializer).length,2,'Production checker loop changed');
const transformed = original.replace(initializer,()=>singleton);
assert.equal(transformed.split(singleton).length,2);
assert.deepEqual(Buffer.from(transformed.replace(singleton,()=>initializer)),Buffer.from(original),
  'The single engine initializer must be the only checker change');
assert.equal(digest(original),materialization.sourceFiles['browser-single/check-pages.mjs'].sha256);
const copiedPath = path.join(source,'browser-single/check-pages.singleton-'+engine+'.mjs');
await fs.writeFile(copiedPath,transformed,{flag:'wx'});
const syntax = spawnSync(process.execPath,['--check',copiedPath],{cwd:source,stdio:'inherit'});
assert.equal(syntax.status,0,'Singleton checker syntax failed');
const transformProof = {
  sourceCommit,engine,original:{bytes:Buffer.byteLength(original),sha256:digest(original)},
  transformed:{bytes:Buffer.byteLength(transformed),sha256:digest(transformed)},
  copiedRelativePath:path.relative(source,copiedPath),inverseByteComparison:true,
  onlyChange:'literal engine loop initializer',maxVmStarts:1,
  firstWaitMilliseconds:1200000,secondWaitMilliseconds:180000,
  modelExplorerBuildVerified:false,diagnosticOnly:true
};
await fs.writeFile(path.join(evidenceDirectory,engine+'-checker-transform.json'),JSON.stringify(transformProof,null,2)+'\n');
console.log('BOOTSTRAP_BROWSER_GATE '+JSON.stringify({phase:'checker_launch',...transformProof}));
const startedAt = new Date().toISOString();
const child = spawn(process.execPath,[copiedPath],{cwd:source,env:process.env,stdio:'inherit'});
const outcome = await new Promise(resolve=>{
  child.once('error',error=>resolve({code:null,signal:null,launchError:String(error)}));
  child.once('exit',(code,signal)=>resolve({code,signal}));
});
const resultDirectory = path.join(source,'artifacts/browser-wasm');
let result;
let readError;
try {
  const filename = path.join(resultDirectory,engine+'.json');
  assert((await fs.stat(filename)).size <= 2*1024*1024,'Unexpected result JSON size');
  result = JSON.parse(await fs.readFile(filename,'utf8'));
} catch (error) { readError = String(error); }
const files = {};
let consoleText = '';
for (const suffix of ['-console.txt','-stream.jsonl','.json']) {
  const filename = path.join(resultDirectory,engine+suffix);
  try {
    const limit = suffix==='-stream.jsonl' ? 128*1024*1024 : suffix==='-console.txt' ? 16*1024*1024 : 2*1024*1024;
    assert((await fs.stat(filename)).size <= limit,'Unexpected evidence size: '+suffix);
    const bytes = await fs.readFile(filename);
    files[engine+suffix] = {bytes:bytes.length,sha256:digest(bytes)};
    if (suffix==='-console.txt') consoleText = bytes.toString('utf8');
  } catch (error) {
    files[engine+suffix] = {error:String(error)};
  }
}
let validationError;
try {
  assert.equal(outcome.launchError,undefined);
  assert.equal(outcome.signal,null);
  assert.equal(outcome.code,0,'Production checker failed');
  assert.equal(readError,undefined);
  assert.equal(result.engine,engine);
  assert.equal(result.passed,true);
  assert.deepEqual(result.pageErrors,[]);
  assert.equal(result.logCapture?.complete,true);
  assert.equal(result.guest?.error,undefined);
  assert.equal(result.guest?.booted,true);
  assert.equal(result.guest?.ready,true);
  assert.equal(result.guest?.accepted,true);
  assert.equal(result.guest?.checks,2);
  assert.deepEqual(result.passes,result.guest.passes);
  assert.equal(result.passes.length,2);
  for (let i=0;i<2;i++) {
    assert.equal(result.passes[i].id,i+1);
    assert.equal(result.passes[i].accepted,true);
    assert.deepEqual(result.passes[i].statuses,[403,204,200,204,403]);
  }
  assert(Object.values(files).every(file=>!file.error),'Required full evidence file is missing');
} catch (error) { validationError = String(error); }
const observations = consoleText.split(/\r?\n/).filter(line=>
  /LAB\||\[browser-(guest|single|openfga)\]|Listening on|database system is ready|TimedOut|Elapsed\(\(\)\)|context canceled|error_kind|Error:|FATAL|PANIC/.test(line));
const excerpt = observations.slice(-40).map(line=>line.length>1500?line.slice(0,1500)+' [truncated; see full console]':line);
const summary = {
  schemaVersion:1,experiment:'bootstrap_model_budget_30',diagnosticOnly:true,
  sourceCommit,workflowCommit:process.env.GITHUB_SHA ?? null,engine,
  maxVmStarts:1,startedAt,finishedAt:new Date().toISOString(),outcome,
  browser:result?.browser ?? null,
  twoRealSequencesPassed:validationError===undefined,
  passes:result?.passes ?? null,guest:result?.guest ? {
    booted:result.guest.booted,ready:result.guest.ready,accepted:result.guest.accepted,
    checks:result.guest.checks,error:result.guest.error ?? null,vm:result.guest.vm
  }:null,
  pageErrors:result?.pageErrors ?? null,logCapture:result?.logCapture ?? null,
  failure:result?.failure ?? null,readError:readError ?? null,validationError:validationError ?? null,
  files,sourceTransformation:transformProof,
  bootstrapBuild:materialization.bootstrapBuild,
  data:materialization.data,loader:materialization.loader,kernel:materialization.kernel,
  rootfsUnchanged:materialization.rootfsUnchanged,
  excerptScope:'Raw matching console lines may include diagnostic-tail replays; not fresh RPC results or an independent pass count.',
  matchedObservationLines:files[engine+'-console.txt']?.error ? null : observations.length,excerpt
};
await fs.writeFile(path.join(evidenceDirectory,engine+'-summary.json'),JSON.stringify(summary,null,2)+'\n');
console.log('BOOTSTRAP_BROWSER_GATE '+JSON.stringify(summary));
if (validationError!==undefined) process.exitCode=1;
