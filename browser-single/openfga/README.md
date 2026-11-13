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

## Bounded structural-path reuse in build .9

Build `v1.10.3-browser-startup.9` prepares a private read-only map of the
original `PathExists` predicate for the plain `user` type and known schema
object-type/relation pairs. The pinned model contains 316 such pairs; preparation
has a 512-pair bound and remains inside the existing 60-second startup phase.
Each entry is computed by the original method, including its wildcard handling.
Cancellation or an error prevents publication of a partially filled map.

The map belongs only to the privately prepared startup TypeSystem. Its existing
one-shot transfer still requires an actual persisted model read and an exact
body match. All real Check rewrites, tuple reads, conditions and authorization
decisions remain. Entity IDs and authorization answers are not retained.
Usersets, other subject types and unknown target keys use the original method
and errors. Models without a `user` type or above the bound keep that path too.
The existing resolver-cache lifetime and fallback construction are unchanged.

[Fresh native verification 43478795296](https://github.com/qqrm/openfga-axum-lab/actions/runs/43478795296)
passed the complete TypeSystem tests, the existing command/server regressions,
and focused race checks. Four new tests cover all 316 predicate pairs against
the original method, eligibility and bounds, failure/cancellation without
partial publication, and concurrent read-only access. The versioned producer
also passed eight Node and 25 Rust tests, a real PostgreSQL/OpenFGA/Axum proof
and two original `[403,204,200,204,403]` sequences.

The actual empty-store Check returned HTTP 200 with `allowed:false` and
`HIGHER_CONSISTENCY`. Holding its genuine response for six seconds produced
exit 1 after 5089 ms with no published configuration. Default model POST and GET
likewise failed closed after 5068/5072 ms while their real responses remained
withheld. PostgreSQL contained four models and no tuples before app startup.

The tested OpenFGA is 63074488 bytes, SHA256
`7841a91b524fc1be097f2f816a8ed39f502e1372bbf96300dc9008b3336f98c3`.
The bootstrap is byte-identical to the previous tested binary.
Native warmup took 57 ms, with 49 ms in TypeSystem validation and path
preparation. These timings do not establish WASM performance. Browser acceptance
for .9 is still pending; the last completed .8/RNG run failed real Check
deadlines in both engines. See the [current browser review](../../docs/browser-startup-review.md).

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
binary encoding/decoding, generated `ValidateAll` request validation and the complete original
TypeSystem validator. It then encodes local write-model and full read-model
response fixtures through protobuf and JSONPb. The fixed ULID is codec input
only. No request handler, datastore, store/tuple write or Check is called, and
no response is sent.

After every stage succeeds, the caller publishes an opaque, private deep copy
of the successfully validated **conditionless schema body**. The copy excludes
only `AuthorizationModel.Id`. Every other field, nested message and unknown
field participates in `proto.Equal` for subsequent matches. The pinned protobuf
descriptor is tested so that adding a future field cannot silently broaden the
match. No caller receives a reference to this private copy, and it does not
alias the separately retained TypeSystem.

The real write-model command previously built a TypeSystem solely for validation
and immediately discarded it. For an exact match it now reuses the established
validity; generated request validation, type/size limits, new model ID and the
actual PostgreSQL write remain in place. All body mismatches, absent cache
entries and conditional models use the complete original validator. No
authorization answer, tuple or datastore existence is memoized.

Browser builds since `.8` also retain the fully validated warmup TypeSystem in a
private atomic slot, separately from the cloned comparison key. The first
matching resolver cache miss can claim it only **after the existing resolver
accessor has returned the actual model**. The claim requires a nonempty store
and resolved model ID, equality of that ID with the returned model, and the
complete pinned body comparison. An atomic swap removes the object from the
slot permanently; one caller receives exclusive ownership. Its actual model ID
is set before it escapes. The pinned graph builders do not encode that model
ID in their graph state.

Context checks before and after the claim preserve cancellation; a claimed
object is discarded on cancellation rather than returned or reinserted.
Other callers retain `NewAndValidate`: a matching conditionless model gets
a fresh `New(model)`, while every other model gets the complete validator.
Independently constructed TypeSystems do not share this retained object's
mutable state or comparison key. Upstream's store/model-bound resolver cache
remains. Its model accessor may use an existing datastore cache; this does
not promise a physical PostgreSQL query on every call.

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

## Exact pinned-model JSON decoding

[Run 43378543550](https://github.com/qqrm/openfga-axum-lab/actions/runs/43378543550)
started PostgreSQL and OpenFGA in Chromium and Firefox. Both completed warmup,
self-dial, Health and CreateStore; both failed the initial model POST at the
unchanged five-second bootstrap limit. HTTP start to unary observer took
4780 ms in Chromium and 6450 ms in Firefox. Generated validation and auth run
after that observer. Warmup's separate JSON decode stage took 3970/7440 ms;
this supports examining the decoder but does not attribute the entire request
interval to JSON. Both browsers completed zero acceptance passes.

The pinned gateway's `JSONPb.NewDecoder` first reads a `json.RawMessage` with
the standard streaming JSON decoder, then invokes `protojson.Unmarshal`.
The browser codec retains the first step unchanged. Only an exact raw-byte
match of the pinned model can replace the second step with an independent
protobuf copy of the already decoded request. Internal whitespace changes,
different fields, unknown fields and malformed bodies use the original
decoder and its original errors. Other request types and decoder options keep
the original JSONPb behavior. The separate, narrowly guarded response encoder
is described below; HTTP-body handling remains unchanged.

The ordinary JSONPb decoder prepares a private prototype during the existing
bounded startup phase. The caller receives this codec only after every warmup
stage succeeds; no global codec entry or partially initialized gateway is
published. Warmup uses an independent request before setting its fixture ID.
Each real request receives another independent copy, with no fixture store ID.
The generated gateway still supplies the actual `store_id` from the URL and
calls the real gRPC client. Validation, authentication, context/deadlines, model
size checks, real model ID generation and PostgreSQL writes remain mandatory.
This decoder entry stores no response, store existence, tuple or authorization
result.

One fixed diagnostic event records decode duration, byte count, cache hit and
failure state without the JSON body. The native parity and real HTTP/gRPC
integration tests are separate from the required browser acceptance gate.
The candidate keeps the two/three/five-second request limits unchanged.

The retained image's actual `lab-tool` executable was also run natively against
the generated HTTP gateway, loopback gRPC and OpenFGA's real memory datastore.
Its successful bootstrap sent exactly **52708 bytes**, SHA-256
`ac5325a6b1d41ec045393b3ccba88bfcb2b016c9cfa19a31773e149c7d152a59`,
matching the private raw key byte for byte. The model was read back through the
real service. Executable SHA-256:
`f11c71c16a1b74063a7a1c2a5ea72601efe165bc5cff327a732e68bd988999b8`.
This wire proof uses the retained binary, rather than a reimplementation of
Rust serialization. It is optional in the native test suite through
`BROWSER_RETAINED_LAB_TOOL`; the ordinary builder does not have that image's
executable and explicitly skips this one check. This is not PostgreSQL or
browser acceptance.

## Reuse within a real tuple Write

[Run 43353568819, commit c6eb256f](https://github.com/qqrm/openfga-axum-lab/actions/runs/43353568819)
used the one-cycle schema cache and the QEMU 15000/2048 candidate. Chromium
completed warmup in 9.77 seconds and self-dial in 350 ms. The actual model write
succeeded: its RPC took 1.22 seconds and its HTTP request 3.36 seconds. Bootstrap
then verified and persisted the real store/model binding. The application
started but failed before its listener during the first tuple Write of SQL
authorization migration: RPC 4.74 seconds, HTTP 4.91 seconds, OpenFGA code 4004
(`deadline_exceeded`). Firefox separately failed with a browser WebAssembly
out-of-memory error. Neither browser completed an acceptance pass.

The pinned `server.Write` first resolves a TypeSystem and checks write
authorization. Its `WriteCommand.validateWriteRequest` then reads the same model
and constructs another TypeSystem, including both graphs. The schema-validation
cache alone does not remove this second construction. The transport trace does
not measure the two constructions separately, so it cannot attribute the whole
4.74-second interval to them.

The browser patch now passes the already-resolved TypeSystem to that request's
WriteCommand with its store ID. Reuse requires the same nonempty store and model
ID and a supported schema version. Nil, mismatched or unspecified bindings take
the original read-and-construct path. The server still resolves the actual model
and checks authorization before providing the option. Every tuple/condition,
implicit-relation, duplicate, size and deletion check remains, followed by the
actual datastore write with the original request context and error handling.

This same-request Write reuse does not itself add a global TypeSystem cache.
The separate one-shot warmup transfer described above is consumed by one
resolver call. Upstream already caches models by
store/model because model bodies are immutable: the API creates a new ID and
PostgreSQL inserts the model. The second model read was already allowed to hit
that cache without SQL. Reusing the TypeSystem from the same resolved request
does not replace any Check or return a successful write without its datastore
operation. Full command tests, including the original validation cases, and
the existing server Write/authorization/schema tests guard this path.

The same-request optimization has not established browser acceptance. Before
`.8`, a cold resolver still built one TypeSystem; the one-shot transfer targets
that first matching construction. Subsequent real migration Checks, readiness
and acceptance remain required. The same earlier Chromium trace contains
full-model HTTP reads of 2.79, 2.61 and 2.17 seconds; the application's HTTP
readiness adapter retains its two-second request deadline. This is a possible
later bottleneck, not the failed operation in that run. Firefox memory pressure
also remains unresolved. No startup, OpenFGA request, application authorization
or CLI deadline is increased by this reuse.

[Run 43358719645, commit e2090100](https://github.com/qqrm/openfga-axum-lab/actions/runs/43358719645)
tested this reuse together with service affinity to CPU 0. Chromium completed
warmup in 16.890 seconds, self-dial in 180 ms and the real model write in
4.030 seconds. The first tuple Write failed with code 2000 (`validation_error`),
RPC 3.440 seconds / HTTP 3.710 seconds. The trace omits the status message and
therefore does not prove that a tuple or model was invalid. In the pinned
resolver, a context error from `NewAndValidate` is wrapped as `ErrInvalidModel`
and then mapped to code 2000. The next diagnostic build labels only the two
exact resolver messages for deadline/cancellation and the RPC context state;
it never prints arbitrary error messages or changes the returned error.
Service affinity is removed separately, restoring scheduling across both CPUs.
Firefox again failed during PostgreSQL initialization; both browsers recorded
zero acceptance passes. The final evidence ZIP SHA-256 is
`339bad2536bfdd6d88ec4246fcafc6929366958bbb54e354912c4fc9c91f90df`.

## Exact encoding of one real model-read response

The completed [run 43434436745](https://github.com/qqrm/openfga-axum-lab/actions/runs/43434436745)
used browser build `.7` with the separately verified bootstrap executable.
Both browsers completed initial model provisioning and recorded **zero of the
two required acceptance passes**. Chromium's first tuple Write failed with
`context_state=deadline` and `error_kind=model_resolution_deadline`: RPC
about 3300 ms, HTTP 3814 ms. That observation motivates the one-shot resolver
transfer; it does not measure graph construction alone.

Firefox completed bootstrap model POST and verification GET under their
opt-in 30-second client budget, then entered the original Axum startup path.
The application's separate `ReadAuthorizationModel` RPC succeeded in 56 ms,
while HTTP ended canceled after 5644 ms under the unchanged five-second
application deadline. The approximately 5.44-second interval after RPC
completion includes gateway protobuf decoding, JSON encoding, writing and
scheduling. It does not identify one serialization function as the cause.
The [retained review](../../docs/browser-startup-review.md) records the
individual operation timings and their limits.

The generated gateway still calls the real gRPC client and handles its errors
before forwarding a successful response. Its `ForwardResponseMessage` calls
the HTTP marshaler's `Marshal` method. Browser builds since `.8` intercept only
`ReadAuthorizationModelResponse` at that point and can retain **one eligible
successful serialization, at most 1 MiB**, per codec instance. Admission requires
the pinned conditionless model, its actual nonempty valid model ID, no
unsupported response/body fields, and the exact intended JSONPb options.
Unsupported cases use the original marshaler.

The encoder first obtains the successful bytes from the original JSONPb
implementation, then stores a private deep protobuf key and a private byte
copy with a single compare-and-swap. It neither substitutes a warmup ID nor
creates a response before a real read. Every hit requires `proto.Equal` on
the **entire newly returned response**, including its actual model ID and
unknown fields. A hit returns another private byte copy. Mutation of input
or returned bytes cannot change the stored entry; concurrent first responses
cannot replace an already published entry.

A different model or ID, different options, unsupported response type,
oversized encoding or encoding error keeps the original path. All eight
exported `protojson.MarshalOptions` fields are guarded against the intended
settings, and a descriptor-style reflection test detects additional fields.
`NewEncoder` remains the original JSONPb method, so warmup's response fixture
cannot populate the entry. The real RPC/model retrieval still executes on
every request reaching this marshaler; the existing model cache may satisfy
an accessor without physical SQL. No authorization decision, missing-model
result, auth failure or canceled RPC becomes a successful cached response.

Each eligible response path emits a fixed `model_encode` event with encoding
duration, byte count, cache-hit/store flags and failure state, without payload
or identifiers. This measures the wrapped encoding path, not the entire
post-RPC HTTP interval. The first eligible read pays the extra bounded clone
cost. Reusing its encoding is a testable latency hypothesis, not an established
Firefox fix or a reason to change application, migration, OpenFGA request or
acceptance deadlines.

The first `.8` native attempt,
[43439983302](https://github.com/qqrm/openfga-axum-lab/actions/runs/43439983302),
stopped at `gofmt` before Go/race/native-service execution. The format-only
correction was followed by [43440821229](https://github.com/qqrm/openfga-axum-lab/actions/runs/43440821229),
whose actual OpenFGA regular and focused race tests passed, including the new
encoder and resolver-transfer cases. Its later native-service proof stopped
at the original executable-availability guard. The producer now stages the
verified executable with traversable directory permissions and rechecks its
hash, preserving the unprivileged service user and original input.

[Native verification 43445129977](https://github.com/qqrm/openfga-axum-lab/actions/runs/43445129977)
then reused the identical Go-tested `.8` binary and completed the real
PostgreSQL/OpenFGA/Axum proof: default-five-second failure cases, opt-in
30-second provisioning with six-second-delayed genuine responses, and two
consecutive native acceptance sequences. The original input directory was
deliberately 0700 and remained unchanged. The
[retained review](../../docs/browser-startup-review.md#current-8-candidate-and-completed-native-verification)
records counts, binary/artifact hashes and the earlier failure's limits.
Native success does not establish browser acceptance. The current PR's
**Browser preview** checks remain authoritative for the required two genuine
sequences in each Chromium and Firefox.

## Real bootstrap deny probe before application migration

The separately pinned bootstrap overlay now offers one opt-in real Check
after it has provisioned and read back the original model and confirmed that
the new store is empty. It requests the original direct `ownership` relation
for a probe user/object at `HIGHER_CONSISTENCY`, with actual store/model IDs.
It requires HTTP 200 and `allowed:false` before atomically publishing the
binding. It creates no tuple, retries no uncertain operation, and uses the
ordinary five-second client/body deadline and this server's three-second
request deadline. Existing bindings and the ordinary CLI retain their previous
behavior.

For this fresh browser store, the probe is expected to make the first real
resolver call and claim the prepared TypeSystem described above, filling the
existing store/model cache before the first migration Write. The Check handler,
gateway and direct datastore read really execute. This does not warm every
union or tuple-to-userset path and does not replace any subsequent Check.

[Native verification 43456493622](https://github.com/qqrm/openfga-axum-lab/actions/runs/43456493622)
passed with the unchanged `.8` OpenFGA binary: eight payload tests, 25 Rust
bootstrap tests, real HTTP 200 / `allowed:false`, zero PostgreSQL tuples before
app startup and two original native acceptance sequences. With the genuine
Check response withheld for six seconds, the bootstrap exited 1 after 5086 ms
without publishing configuration. This demonstrates the preserved operation
deadline, not browser latency. The [current browser review](../../docs/browser-startup-review.md)
records actual engine outcomes and all source/binary bindings.

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
- Model decoder parity covers exact/deeply independent copies, changed and
  malformed JSON, unknown/duplicate fields, streaming/read-ahead and reader
  errors, other request types/options, HTTP-body encoding and panic behavior.
  The real HTTP/gRPC integration compares default and cached decoding with
  model writes/readback, separate store bindings, auth failures, invalid models
  and cancellation without a datastore write.
- Response-encoding tests compare original JSONPb bytes, actual-ID/body/unknown
  field and option fallbacks, failed/oversized encodings, independent key/output
  copies and concurrent single-entry publication. The real HTTP/gRPC tests prime
  a successful entry, then require ordinary auth, wrong-store, missing-model and
  canceled-request failures to retain their original behavior. Real RPC/model
  retrieval is observed; this does not assert physical SQL on each request.
- Exact-schema and one-shot transfer tests cover generated IDs,
  nested/unknown-field mismatches, invalid-model fallback, independent
  inputs/TypeSystems, one concurrent claimant, and cancellation before or after
  a claim. Generic calls cannot consume or refill the slot. Real server
  Write/Check/delete tests compare the original behavior and errors using the
  real memory datastore. No cached value substitutes a Check result.
- Write-command tests cover exact store/model reuse, mismatched/nil/latest-model
  fallback, unsupported schema, original tuple/condition errors, real in-memory
  writes/deletes and duplicate-write failure, datastore failure and concurrency.
  Cancellation tests preserve the original request context and use a
  context-aware adapter over the real memory datastore, whose own Write does
  not check cancellation. These native tests do not prove PostgreSQL behavior.
- The full command suite and existing server Write, write-authorization and
  invalid-schema tests run with the final combined patch. The focused HTTP,
  model and resolver-transfer tests also run under Go's race detector.

The tests include a deliberate 60-second wait to verify the production bound.
They do not substitute for the real PostgreSQL/OpenFGA/Axum browser acceptance.
Both Chromium and Firefox must still pass the actual sequence twice, in order:
`403 → 204 → 200 → 204 → 403`.

Output contains `openfga`, `build.json`, `dial-tests.log`,
`model-validation-tests.log`, `write-typesystem-tests.log`,
`model-reuse-race-tests.log`, `LICENSE`, `SOURCE.txt`, the exact source patch,
injected helpers, and codec, Write-command and resolver-transfer test sources.
`build.json` is created only after passing
tests and records the source commit, compiler version, canonical UTC commit
date, patch/helper/test digests, metadata-only diagnostics, omitted unary logging,
the pinned warmup model identity and budget, native warmup proof, same-request
store/model TypeSystem reuse, exact JSON decoder, one-shot resolver transfer,
one full real-response encoding, native parity/transport and race-test proof,
dependency checksum and executable SHA-256. This proof means the native APIs/tests passed; it is not
browser acceptance. The adapted build identifies itself as
`v1.10.3-browser-startup.9`.

The same `build.sh` can run directly from a clean checkout of the pinned upstream
commit when Go 1.24.6 is available on `PATH`; pass the output directory as its
first argument and, optionally, the fixture path as its second argument. The
default fixture is this repository's `fga/model.json`, resolved relative to the
script. Docker copies that existing file separately for tests; no duplicate model
is committed, embedded in the binary or included in the output. The same source,
dependency, test and build gates apply to both recipes. Syntax checks and patch
applicability alone are insufficient. A successful native build verifies the
startup, warmup and observer tests; real browser acceptance is a separate gate.
