# PR #36: idle-worker finalizer delivery fix

Status: the idle-worker queue regression and complete QEMU build pass. Both
browser acceptance jobs fail with an explicit guest startup error, before the
host timeout. Neither browser completes an acceptance vector. PR #36 remains
unready for merge or Pages publication.

## Change and causal scope

The previous [stall investigation](pr36-vm-stall-findings.md) found a concrete
Chrome signature: a vCPU was halted while its worker owned 11,264 detached JIT
instances whose finalization callbacks had not arrived. Nearly simultaneous
lifecycle-counter snapshots reconstructed the configured 15,000-instance
ceiling; this was not an atomic read of the shared C counter. The older Firefox
timeout did not receive the same causal diagnosis.

The pinned QEMU idle path called `qemu_cond_wait`, which reaches the synchronous
Emscripten pthread/atomic wait. An idle worker can remain inside that call
without returning to its JavaScript event loop. Yielding a different, active
vCPU cannot deliver the idle owner's callbacks.

The candidate adds a pending-finalizer branch to `qemu_wait_io_event`. With
local pending instances, it waits up to 20 ms, rechecks the normal idle
predicate, releases the BQL, yields through `emscripten_sleep(0)`, and reacquires
the BQL. Callback accounting is drained before checking pending work and after
the yield. With no pending work, or before initialization, the existing idle
wait is retained. Kicks during the wait or yield still return control through
the ordinary idle predicate.

The patch does not raise instance caps, release credits before callbacks,
change the active-vCPU dispatch/yield policy, force garbage collection, or
change guest services or authorization deadlines. The 20 ms poll provides an
opportunity to service the worker queue; it is not a deadline for JavaScript
garbage collection or `FinalizationRegistry` delivery.

## Immutable candidate

| Item | Identity |
| --- | --- |
| Baseline runtime source | `444b069daf16e9606a0bbf1ea0aea7f4d61b97d8` |
| Candidate source | [`6f35f24b7ca345cf47c873becee6a41ef94e65a9`](https://github.com/qqrm/openfga-axum-lab/commit/6f35f24b7ca345cf47c873becee6a41ef94e65a9) |
| Diagnostic workflow | `a12b8799fed16e6efc93ab55498e8f165fb1a139` |
| QEMU upstream | `8604ed49a3cde392890b014a8d5a959c8a2fe72a` |
| Emscripten | `4.0.10` |
| Idle patch SHA-256 | `48aa5fcb57790e51f2ad16e6075fc93bf6345c944df88de5b69e8daa45540e9f` |
| Compiled WASM SHA-256 | `bca14ba86cff602c979e3ae93df80611b186ecbe101542ec14e6809c528e0db3` |
| Loader SHA-256, unchanged | `95f7e6b59d98cd7267e705aa0b2cd670970b984d13c7e81b4d4f677cd65534d2` |
| Compiled artifact | `14276419843`, 16,286,946 bytes |
| Artifact ZIP SHA-256 | `6fcd65f16a8a05207b7a15460ae2097ecafa00ee031fcc4c60bda3edf5e1ea30` |

Relative to the baseline, the candidate changes six repository files: the new
QEMU patch and regression checker, QEMU build script, Docker appendix, emulator
materializer, and QEMU README. The production patch itself changes three
upstream files with 51 added lines and one removed line.

The first candidate build, [run 43967883070](https://github.com/qqrm/openfga-axum-lab/actions/runs/43967883070),
failed in the new SDK fixture: comma-separated JavaScript declarations inside
`EM_ASM` were split by the C preprocessor. Commit `6f35f24` changes those
declarations to separate statements. The QEMU patch was unchanged, and no
browser VM was launched by the failed build.

## Completed verification

[Run 43969702163](https://github.com/qqrm/openfga-axum-lab/actions/runs/43969702163)
successfully built the complete QEMU pair. Its build job is `133658307719`.
All existing build validations and the new native/SDK proofs passed. The
artifact digest, source inputs, manifests and compiled pair were independently
checked after download.

The eight native cases cover queued work, no pending work, already delivered
callbacks, kicks during the wait and during the yield, uninitialized state,
the original blocking behavior, and rejection of a yield that keeps the BQL.
The six SDK cases use actual Emscripten 4.0.10 pthreads and Asyncify. The candidate
delivers the queued task before an external kick; the baseline delivers it only
after a kick and an explicit event-loop yield. The locked-yield negative control
is rejected. The queued callback payload is a deterministic test double, not a
real `FinalizationRegistry` collection.

The SDK proof manifest has SHA-256
`434ef8628d8a1c6367f7e71843fd7c1cc3c8c2d276b063352f40f2302daa3a88`.
The proofs also verify byte-identical instance-budget, active-dispatch and
active-yield source regions against the preimage.

The complete WASM contains the new path, rather than only the test fixture:
in function `qemu_wait_io_event` (index 6106), code-section decoding found direct
calls to `wasm32_has_pending_gc` (10760, body offset +360),
`qemu_mutex_unlock_impl` (+743), and `wasm32_yield_for_gc` (10761, +768).
The body also contains the 20 ms constant. These are decoded instructions, not
matches in debug strings.

## Browser acceptance

Both jobs in run `43969702163` finished with `LAB|boot-failed`. Each final guest
record is `booted=true`, `ready=false`, `accepted=false`, `checks=0`, `passes=[]`.
Both have `pageErrors=[]` and a complete source log capture. These failures were
reported by the guest; neither is the previous 20-minute host timeout.
The same class of Check-deadline failure was already recorded in the
[earlier .8 run](https://github.com/qqrm/openfga-axum-lab/actions/runs/43476279151).
This experiment reaches that known boundary again with the new QEMU candidate;
it does not discover the three-second request limit for the first time.

| Browser | Successful work before failure | Terminal failure |
| --- | --- | --- |
| Chrome `157.0.8103.50`, job `133662483116` | Warmup 16,474 ms; self-dial 419 ms; bootstrap empty-store Check 2,005 ms; Axum process reaches SQL and authorization migration; first migration Check 2,845 ms | Next migration Check returns `Code(4004)`, `context_state=deadline`, 4,380 ms. HTTP finishes after 5,399 ms with `context canceled`; Rust reports `reqwest::Error … TimedOut`; the required service exits with status 1. |
| Firefox `159.0`, job `133662483218` | Warmup 19,584 ms; self-dial 864 ms; health, model POST/GET and tuple read succeed | First bootstrap Check returns `Code(4004)`, `context_state=deadline`, 3,132 ms. HTTP finishes after 3,421 ms; bootstrap receives HTTP 500 instead of 200 and exits before Axum starts. |

These request durations are the guest service's recorded durations. They are
not host receive timestamps and must not be mixed with the table below.

The owner of the inner deadline is OpenFGA's unchanged 3-second request
timeout. The pinned [server configuration](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/pkg/server/config/config.go)
sets `DefaultRequestTimeout`; the [timeout interceptor](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/pkg/middleware/timeout.go)
wraps the handler with `context.WithTimeout`. The HTTP gateway's fallback is
6 seconds. Bootstrap Check and the Rust migration HTTP client retain 5-second
budgets. Cooperative deadline handling can finish after the configured deadline;
the Chrome 4,380 ms RPC does not mean that its configuration was raised.

In Firefox the original log also records `model_resolver_transfer claimed=true`
inside the failed Check. The prepared TypeSystem was obtained; a fresh complete
`NewAndValidate` is therefore not established as the cause of that request.

For Chrome, source order provides a narrower inference: migration visits users
in SQL ID order, then a `BTreeMap` of objects, then the declared action list.
Its first pair is `user:alice`, `lakekeeper_table:analytics/errors`, with
`can_get_metadata` followed by `can_include_in_list`. Their serialized body
lengths are exactly 199 and 202, matching the two observed requests. The relevant
Rust files are byte-identical to rootfs source `99f00cc…`. This identification is
derived from the deterministic source/seed order, not from a logged request
payload. In the pinned model `can_include_in_list` is an alias of
`can_get_metadata`; the observations do not justify a new schema cache or an
authorization shortcut.

### Host event timeline

These are `receivedAt` timestamps in UTC of the first original source markers.
Diagnostic-tail replays are excluded. Starting the Axum process does not mean
the application has become ready or started accepting requests.

| Event | Chrome | Firefox |
| --- | --- | --- |
| PostgreSQL initialization | 22:34:38.943 | 22:35:41.014 |
| PostgreSQL server started | 22:38:28.199 | 22:40:14.138 |
| OpenFGA migration/start | 22:39:07.614 | 22:41:11.987 |
| Warmup complete | 22:40:04.872 | 22:42:32.540 |
| Self-dial complete | 22:40:07.152 | 22:42:35.058 |
| Model binding | 22:40:10.582 | 22:42:39.540 |
| Axum process start | 22:40:28.284 | Not reached |
| Failing Check result | 22:43:48.824 | 22:43:07.659 |
| `LAB\|boot-failed` | 22:44:12.037 | 22:43:27.592 |

The gate uses the exact previous runtime materialization and a verified
emulator-only overlay. All 17 native inputs are preserved. It retains 2 vCPU,
512 MiB guest RAM, the original 20-minute first wait and 3-minute second wait,
all 59 production assertions, and two distinct authorization passes with
statuses `[403, 204, 200, 204, 403]`. Chrome uses the installed system binary
with its sandbox enabled; Firefox comes from the locked Playwright runtime.
The wrapper launched at +67.183 seconds for Chrome and +113.169 seconds for
Firefox from the actual GitHub job start. First QEMU source arrived at +71.786
and +120.335 seconds respectively, both below the 240-second launch guard.
No HMP, worker evaluation or additional guest diagnostic injection ran during
acceptance; the existing guest's failure snapshot remains part of the input.

### Remaining JIT/GC uncertainty

Firefox continues compilation and finalization during the failed request.
Between host receives 22:43:04.521 and 22:43:07.659, core 0 registrations rise
287,744 → 289,792, core 1 registrations 308,224 → 309,248, and core 1 GC callbacks
304,128 → 307,200. It is doing observable work while exhausting the deadline.
Its maximum gap between nonempty source batches is 13.771 seconds.

Chrome's last regular JIT snapshots are at 22:39:50.857–22:39:50.900, roughly
4 minutes 21 seconds before the final guest failure, despite later SQL/HTTP/RPC
progress. Its largest source-batch gap is 151.009 seconds during Axum startup
(JSONL lines 415–416). The sparse counters do not establish the exact remaining
global budget or whether compilation stopped entirely. The idle fix must not
be described as eliminating every Chrome JIT/GC stall.

| Last regular snapshot | Registered | Detached | GC callbacks |
| --- | ---: | ---: | ---: |
| Chrome core 0 | 84,992 | 82,944 | 78,848 |
| Chrome core 1 | 59,392 | 57,344 | 51,200 |
| Firefox core 0 | 300,032 | 297,984 | 297,984 |
| Firefox core 1 | 321,536 | 319,488 | 316,416 |

These are separate, non-simultaneous observations. The final source silence is
only 123 ms in Chrome and 60 ms in Firefox, consistent with explicit guest
failure rather than an unresponsive-page timeout.

### Retained evidence references

| Artifact | ID | ZIP SHA-256 |
| --- | --- | --- |
| Chrome | `14279276610` | `b51d264c3539240462283fbe249f17b58b42905ee8507e6ee828e641172cd6a5` |
| Firefox | `14279046719` | `ec9a4e3e6a39cb13c2518aeff2c2d5164d64a7d9630442d4a07b10e8c4c49c81` |

The [Chrome analysis](pr36-idle-gc-evidence/chromium-stream-analysis.json) and
[Firefox analysis](pr36-idle-gc-evidence/firefox-stream-analysis.json) retain exact
source-line references, parsing scope, per-core counter observations, actual
launch budgets and hashes of the original result/console/stream files. They
are derived analyses; the raw logs remain in the linked run's artifacts.

## Interpretation limits

Regular JIT telemetry is emitted every 1,024 successful registrations. It can
show later increases in per-core `gc_total` and continued registrations, but it
cannot identify the moment a callback ran or establish that the vCPU was then
in HLT. Do not sum stale observations from different cores as an atomic reading
of the shared budget. Diagnostic-tail replays do not count as fresh service
events or additional authorization passes.

A successful browser gate establishes this candidate's behavior in that run,
alongside the controlled queue regression. It does not establish a guaranteed
finalization deadline, eliminate future instance pressure, prove a performance
improvement, or assign all prior Chrome and Firefox failures to one cause.

## Production integration boundary

The PR head remains `55fb279741aa47ae813e5b7d5aff73dbd2692cc1`. The diagnostic
run does not by itself make the production `Browser preview` status pass.
The candidate also includes the previously prepared runtime `.10` changes;
cherry-picking only the idle patch onto that older PR head would test a different
runtime.

The direct production path in source candidate `6f35f24` has packaging gaps:
its original runtime artifact `13977709673` is no longer available and the
published `runtime-source.zip` fallback returns 404. The diagnostic gate instead
verifies the original ZIP inside retained artifact `14029511154`.

These runtime-recovery and guest-fixture gaps are already addressed in the
previously prepared production integration
[`deabc1bff5eb365a4c7049d50a722dbebe26cdef`](https://github.com/qqrm/openfga-axum-lab/commit/deabc1bff5eb365a4c7049d50a722dbebe26cdef),
based on `03456091800ea1af57a4025678d72980d542b63b`. That production preparation
is distinct from both the published PR head and the tested diagnostic source.
Its verified retained-archive step and fixture-compatible guest must be
preserved when integrating the emulator change. The kernel reuse pin remains
older than the tested VirtIO RNG recipe in both versions, causing a rebuild
instead of reuse. Carrying this result forward requires the exact tested inputs
and browser configuration, with honest producer provenance. Existing retained
inputs expire on November 20, 2026; this report does not extend their retention.

An [unapplied recovery patch](pr36-production-recovery.patch), based on `6f35f24`,
prepares only the exact retained-runtime recovery and checked kernel reuse.
Its workflow change is already present in `deabc1b`; only the kernel resolver
portion is additional work when that existing production base is used.
It passed shell syntax, actionlint, whitespace checks and isolated negative
paths. It has not undergone a full restoration or production workflow run;
ShellCheck was not available locally. It does not change the observed browser
result. Three syscall diagnostic lines in the tested candidate guest differ
from the strict bootstrap fixture. The diagnostic materializer verifies the
exact inverse of that addition. Production `deabc1b` already removes those
lines and preserves the direct fixture comparison in `bootstrap/build.sh`.

## Next bounded investigation

The remaining measured failure is the real OpenFGA Check deadline. The next
diagnostic should identify work within that request, with the current model,
tuples, request limits and authorization semantics preserved.

The smallest first addition can extend the existing `rpc_end` record after the
handler returns with available, allowlisted context tags, recording missing
values explicitly. The pinned Check command creates an internal placeholder
response and returns request metadata even when `ResolveCheck` returns an
error. This lets `Server.Check` set `dispatch_count`, `datastore_query_count` and
`request.throttled` before its external `return nil, finalErr`. Earlier validation
or model-resolution errors can leave those tags absent. The datastore count is
a partial operation counter, not proof of successful SQL statements; the
placeholder's `Allowed:false` is not an authorization decision.

Those tags are not currently included in the saved browser observer logs.
The command also already measures `ResolveCheck` duration in its internal
resolution metadata; exporting that measurement would avoid another clock
measurement inside the resolver. SQL durations and schema-path cache hit/miss
counts are not provided by these tags and need additional instrumentation if
that narrower measurement calls for them.

Phase timing should then separate model resolution from Check execution. Any
new diagnostic must write its summary after the request rather than printing
inside the hot path. A specific graph, SQL or scheduling optimization should
follow that measurement; request wall time and GC activity alone do not identify
which portion needs changing. Neither the fixed idle-queue regression nor this
failed browser run justifies increasing deadlines or bypassing authorization.
