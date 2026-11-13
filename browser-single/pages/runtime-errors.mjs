function errorMessage(value, fallback) {
  if (typeof value?.message === 'string' && value.message) return value.message;
  return value == null ? fallback : String(value);
}

// Keep browser reporting intact while making uncaught runtime failures terminal
// for the experiment. Emscripten can forward a worker ErrorEvent as the error.
export function observeRuntimeErrors(target, fail) {
  const onError = event => fail(errorMessage(event.error, event.message || 'Uncaught browser runtime error'));
  const onRejection = event => fail(errorMessage(event.reason, 'Unhandled browser promise rejection'));
  target.addEventListener('error', onError);
  target.addEventListener('unhandledrejection', onRejection);
  return () => {
    target.removeEventListener('error', onError);
    target.removeEventListener('unhandledrejection', onRejection);
  };
}
