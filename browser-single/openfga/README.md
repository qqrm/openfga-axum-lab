# Browser-only OpenFGA startup adaptation

Upstream is OpenFGA v1.10.3 at
[`293893b22a53d8a130a5f7c97cab4bc8e91bcaa4`](https://github.com/openfga/openfga/tree/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4).
Its HTTP gateway starts the gRPC server, then makes a blocking connection to it
with a hardcoded three-second deadline in `cmd/run/run.go`. Slow Linux/WASM
emulation can exceed that startup deadline. The original failure was observed in
a local QEMU WASM experiment. Subsequent real Chromium runs of the adapted build
reached OpenFGA's gRPC and HTTP listeners; full browser acceptance has not passed.

There is no setting for that self-dial timeout in this release.
`OPENFGA_HTTP_UPSTREAM_TIMEOUT` and `OPENFGA_REQUEST_TIMEOUT` govern requests
and do not change it. Disabling HTTP would also disable the existing bootstrap
and HTTP adapter, requiring a larger migration.

The patch moves this connection into a small helper with a **60-second
startup budget** and parent cancellation. It retains `grpc.WithBlock()`, TLS
options and fatal startup failure. No authorization RPC timeout, OpenFGA policy,
PostgreSQL data or Axum behavior changes. The separate browser startup supervisor
must wait for real OpenFGA readiness before invoking bootstrap; increasing this
budget alone cannot extend bootstrap's own 60-second readiness wait.

The browser build also emits short transport diagnostics through a separate
standard Go logger on stderr, with UTC timestamps. HTTP start/end events contain
the method, path without query string, content length, elapsed time and context
error. Unary RPC start/end events contain the method, status code and elapsed
time. It does not log request/response bodies or headers. These events remain
available when OpenFGA's normal logger is set to `warn`, without enabling its
verbose raw payload logs. Self-dial and model warmup also emit start/end phase
events, duration, failure state and their startup budgets.

The HTTP observer passes the original writer, request, body and context to the
handler. The unary observer passes through the original context, request,
response and error. The browser patch removes only registration of
`logging.NewLoggingInterceptor` for unary RPCs: that interceptor serializes
complete protobuf payloads even at `warn` or `none`. Its raw unary payload logs
and internal-error details are consequently unavailable in this build. The
metadata observer still records completion status; other error logs and the
streaming logging interceptor remain. Validation, authentication, authorization,
timeout and recovery middleware stay in place.
Panics continue to upstream recovery and do not emit a false completion event.
Diagnostic logging itself adds work; it is not a performance improvement or a
replacement for successful browser acceptance.

## Bounded startup and exact-schema validation cache

[Run 43306901697, attempt 2](https://github.com/qqrm/openfga-axum-lab/actions/runs/43306901697/attempts/2)
reached healthy OpenFGA and successfully created a store. For the model POST,
the HTTP observer recorded 52,708 bytes and cancellation after 5,600 ms; the unary
RPC observer started only after that cancellation and completed 11,410 ms later
with the cancelled-operation code. The latter interval included unconditional
upstream payload serialization. These boundaries identify an expensive model
path; they do not isolate the cost of parsing, transport, validation or logging.

Before constructing a datastore or opening any listener, the HTTP-enabled
browser build reads `FGA_MODEL`. It accepts only a regular file of **52,709
bytes**, with SHA-256
`a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d`.
A limited reader prevents an oversized input from allocating without a bound.
The checked input is the unchanged repository `fga/model.json`; the real HTTP
client's serialized request is a separate representation.

One startup cycle exercises the pinned grpc-gateway `JSONPb` decoder, protobuf
binary encoding/decoding, generated request validation and the complete original
TypeSystem validator. It then encodes local write-model and full read-model
response fixtures through protobuf and JSONPb. The fixed ULID is codec input
only. No request handler, datastore, store/tuple write or Check is called, and
no response is sent.

After every stage succeeds, the caller publishes an opaque, private deep copy
of the successfully validated **conditionless schema body**. The copy excludes
only `AuthorizationModel.Id`. Every other field, nested message and unknown
field participates in `proto.Equal` for subsequent matches. The pinned protobuf
descriptor is tested so that adding a future field cannot silently broaden the
match. No caller receives a reference to this private copy.

The real write-model command previously built a TypeSystem solely for validation
and immediately discarded it. For an exact match it now reuses the established
validity; generated request validation, type/size limits, new model ID and the
actual PostgreSQL write remain in place. A resolver cache miss still reads the
actual model from the actual datastore. If its body matches, a fresh `New(model)`
builds both graphs, maps and independent mutable state with the real model ID.
All mismatches, absent cache entries and conditional models use the complete
original validator. No authorization answer, tuple, datastore existence or
TypeSystem is memoized by this new cache. Upstream's existing store/model-bound
resolver cache remains unchanged.

This narrow reuse is justified by the pinned `NewAndValidate` implementation:
after `New(model)`, conditionless schema validation does not mutate the returned
TypeSystem. Conditional models are excluded because condition compilation does
retain additional state. Neither identity nor tuple data affects these schema
checks. The two modified schema-validation locations and every added helper are pinned
and included in the build evidence.

Reading/verifying the file, codecs, full validation and publication share
**one 60-second startup budget**, inheriting parent cancellation. Stage records
include interrupted work. A buffered worker result bounds the caller's wait
if synchronous codec/validation work does not promptly observe cancellation.
Only the caller can publish the cache after a successful wait; a late worker
cannot publish either the cache or readiness. Cancellation wins over late
success. Bad identity, validation failure, timeout or cancellation stops startup
before listeners. The CLI exits on error. A synchronous worker may finish later
before process exit; Go does not forcibly stop it.

The previous four-cycle candidate failed the real
[23562aca browser gate](https://github.com/qqrm/openfga-axum-lab/actions/runs/43343577131):
Chromium completed warmup in 53 seconds and self-dial in 550 ms, then exceeded the
unchanged five-second model-write limit. Firefox exhausted the shared 60-second
budget during its third warmup cycle. Neither browser completed an acceptance
pass. Repeating validation did not establish adequate request performance;
this revision removes that repetition without changing request deadlines.
The remaining graph construction and browser JIT/GC costs still require actual
browser verification.

## Build and check

Run after approval for any paid CI use, from the repository root:

```sh
docker buildx build --platform linux/amd64 \
  -f browser-single/OpenFGA.Dockerfile \
  --output type=local,dest=artifacts/browser-openfga .
```

The recipe pins the upstream commit, Go **1.24.6**, and upstream `go.mod`/`go.sum`.
It verifies the source preimage before applying the zero-context unified patch,
runs the targeted tests, and builds a stripped static Linux/amd64 executable.
The exact source checksum guard is mandatory for this zero-context patch; both
patch checking and application explicitly use `git apply --unidiff-zero`. Module checksums are
verified and dependency/config changes fail the build. The compiler image uses
an exact version tag, and the compiler reports that same version before use.

Tests invoke the same helper called by the patched server:

- A real gRPC server delays its HTTP/2 handshake for four seconds. The original
  three-second call fails; the adapted call connects and completes a health RPC.
- An unavailable server still fails at the actual 60-second startup deadline.
- Parent deadline and cancellation still terminate startup.
- OpenFGA request/list deadlines and HTTP gateway timeout defaults are unchanged.
- Transport observers call the original handlers exactly once, preserving
  inputs, HTTP status/headers/body, RPC results/errors, cancellation and panics.
- Diagnostic output contains only boundary metadata, with no request/response
  bodies, query strings, headers or error details.
- The exact pinned model completes the actual warmup APIs without changing its
  bytes. Missing, tampered, oversized, truncated and non-regular inputs fail.
- A deliberately blocked synchronous worker cannot retain the startup caller
  beyond its budget. Parent cancellation/deadline and completion racing with
  cancellation fail closed.
- The patched server entry point rejects an unacceptable model or cancelled
  parent before datastore or listener initialization.
- Exact-schema cache tests cover generated IDs, nested/unknown-field mismatches,
  invalid-model fallback, independent inputs/TypeSystems and cancellation. The
  cache contains validation evidence only and never substitutes a Check result.

The tests include a deliberate 60-second wait to verify the production bound.
They do not substitute for the real PostgreSQL/OpenFGA/Axum browser acceptance.
Both Chromium and Firefox must still pass the actual sequence twice, in order:
`403 → 204 → 200 → 204 → 403`.

Output contains `openfga`, `build.json`, `dial-tests.log`,
`model-validation-tests.log`, `LICENSE`, `SOURCE.txt`, the exact source patch and
all three helpers. `build.json` is created only after passing
tests and records the source commit, compiler version, canonical UTC commit
date, patch/helper/test digests, metadata-only diagnostics, omitted unary logging,
the pinned warmup model identity and budget, native warmup proof, dependency
checksum and executable SHA-256. This proof means the native APIs/tests passed;
it is not browser acceptance. The build identifies itself as
`v1.10.3-browser-startup.4`, rather than claiming to be an untouched release.

The same `build.sh` can run directly from a clean checkout of the pinned upstream
commit when Go 1.24.6 is available on `PATH`; pass the output directory as its
first argument and, optionally, the fixture path as its second argument. The
default fixture is this repository's `fga/model.json`, resolved relative to the
script. Docker copies that existing file separately for tests; no duplicate model
is committed, embedded in the binary or included in the output. The same source,
dependency, test and build gates apply to both recipes. Syntax checks and patch
applicability alone are insufficient. A successful native build verifies the
startup, warmup and observer tests; real browser acceptance is a separate gate.
