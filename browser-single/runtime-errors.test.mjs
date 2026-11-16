import assert from 'node:assert/strict';
import test from 'node:test';
import {observeRuntimeErrors} from './pages/runtime-errors.mjs';

function event(type, fields) {
  return Object.assign(new Event(type, {cancelable:true}), fields);
}

test('uncaught errors reach the failure callback without suppressing browser reporting', () => {
  const target = new EventTarget();
  const failures = [];
  observeRuntimeErrors(target, error => failures.push(error));
  let observed = 0;
  target.addEventListener('error', () => observed++);
  const failure = event('error', {error:new Error('out of memory')});
  assert.equal(target.dispatchEvent(failure),true);
  assert.equal(failure.defaultPrevented,false);
  assert.equal(observed,1);
  assert.deepEqual(failures,['out of memory']);
});

test('forwarded worker ErrorEvents retain their message and missing errors use event.message', () => {
  const target = new EventTarget();
  const failures = [];
  observeRuntimeErrors(target, error => failures.push(error));
  const workerError = event('error', {message:'WebAssembly.Module: out of memory'});
  target.dispatchEvent(event('error', {error:workerError,message:'uncaught exception: [object ErrorEvent]'}));
  target.dispatchEvent(event('error', {error:null,message:'Script error.'}));
  assert.deepEqual(failures,['WebAssembly.Module: out of memory','Script error.']);
});

test('unhandled promise rejections report errors and primitive reasons without suppressing the event', () => {
  const target = new EventTarget();
  const failures = [];
  observeRuntimeErrors(target, error => failures.push(error));
  let observed = 0;
  target.addEventListener('unhandledrejection', () => observed++);
  for (const reason of [new Error('worker initialization failed'),'runtime trapped',undefined]) {
    const failure = event('unhandledrejection', {reason});
    assert.equal(target.dispatchEvent(failure),true);
    assert.equal(failure.defaultPrevented,false);
  }
  assert.equal(observed,3);
  assert.deepEqual(failures,['worker initialization failed','runtime trapped','Unhandled browser promise rejection']);
});

test('unsubscribe removes both handlers without removing other observers', () => {
  const target = new EventTarget();
  const failures = [];
  const unsubscribe = observeRuntimeErrors(target, error => failures.push(error));
  let observed = 0;
  target.addEventListener('error', () => observed++);
  target.addEventListener('unhandledrejection', () => observed++);
  unsubscribe();
  unsubscribe();
  target.dispatchEvent(event('error', {message:'later error'}));
  target.dispatchEvent(event('unhandledrejection', {reason:'later rejection'}));
  assert.deepEqual(failures,[]);
  assert.equal(observed,2);
});
