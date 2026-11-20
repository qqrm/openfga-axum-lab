# PR #36: full-stack continuation after the memory component passed

Date: November 20, 2026. All timestamps in this report are UTC.

**The real services run in the browser, but neither full-stack execution
reached a ready application API. Both variants completed startup, wrote
the original 40 authorization tuples and entered the migration permission
matrix. Memory-backed OpenFGA completed 247 Checks and failed on number 248;
PostgreSQL-backed OpenFGA completed one Check and failed on number 2. Both
failures exceeded the unchanged OpenFGA request deadline and terminated
the Rust application before its HTTP listener was bound.**

The successful isolated memory component remains valid. It does not establish
full application acceptance. Neither of the two required grant/revoke
executions ran in either new full-stack VM.

## Execution and fixed scope

- [Run 44675797208](https://github.com/qqrm/openfga-axum-lab/actions/runs/44675797208),
  [job 136087192171](https://github.com/qqrm/openfga-axum-lab/actions/runs/44675797208/job/136087192171),
  attempt 1.
- Workflow commit: `9785f31adc0af939a6d1c5edac183593fc69c718`.
- Runtime source: `444b069daf16e9606a0bbf1ea0aea7f4d61b97d8`.
- Emulator source: `6f35f24b7ca345cf47c873becee6a41ef94e65a9`.
- Fresh Chromium VMs, each with two x86_64 guest CPUs and 512 MiB, sequentially
  on one Actions runner. The host and downloaded bytes are shared; guest
  processes, stores, databases and VM state are not reused.
- Original Rust application, PostgreSQL, `.10` OpenFGA executable, bootstrap,
  root filesystem, kernel, Lakekeeper authorization model and all seventeen
  native producer inputs remain unchanged.
- The original application/rootfs source is
  `99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58`. A direct git comparison confirms
  `src/`, `fga/` and `migrations/` are byte-identical to the pinned `444b`
  source used for the control-flow and matrix reconstruction below.
- The memory variant changes only the OpenFGA datastore selector and omits
  its SQL migration. PostgreSQL still runs for the application, and both
  original database-creation commands remain in place.
- Both variants add identical phase and exit observations. Application
  migration, its 40-tuple batch, request deadlines, readiness deadline and
  original acceptance conditions remain unchanged.

The [experiment declaration](pr36-stack-continuation-experiment.md) fixed
these conditions before execution. This is a functional comparison, not a
benchmark: fixed order and changing host/cache/load state prevent causal
speed comparisons. One execution per configuration is not a reliability
estimate.

| Full-stack boundary | OpenFGA memory | OpenFGA PostgreSQL |
| --- | --- | --- |
| Linux, PostgreSQL application database | Passed | Passed |
| FGA SQL migration | Deliberately omitted | Passed |
| Real OpenFGA bootstrap and Rust app process start | Passed | Passed |
| Original 40-tuple Write | `OK`, RPC 141 ms | `OK`, RPC 1,588 ms |
| Migration tuple reconciliation | Passed | Passed |
| Completed matrix Checks | 247 of 1,242 | 1 of 1,242 |
| First failed matrix Check | #248, 3,305-ms deadline | #2, 3,685-ms deadline |
| Ready Axum HTTP API | Not reached | Not reached |
| Two original application acceptance executions | 0 of 2 | 0 of 2 |

RPC times describe these individual observations; the table is not a
datastore performance benchmark.

## Memory-backed OpenFGA with the real PostgreSQL application

The checker ran from `04:40:08.611` to `04:48:47.225`, a host interval of
518,614 ms including browser/guest startup, diagnostics and evidence capture.
It returned exit code 1. The result records `booted=true`, `ready=false`,
`accepted=false`, `checks=0`, `passes=[]`, and `LAB|boot-failed`.

| Boundary | Observed result |
| --- | --- |
| Linux and two usable guest CPUs | Confirmed; online and service affinity `0-1` |
| PostgreSQL initialization, server and both databases | Completed |
| Real OpenFGA memory service | Listener ready; original bootstrap completed |
| Rust application process | Started as PID 206 |
| Migration Write | One 3,419-byte request; RPC `OK` in 141 ms, HTTP 302 ms |
| Tuple reconciliation | Completed sufficiently to enter the permission matrix |
| Migration matrix | 247 successful Checks, then a deadline on attempt 248 of 1,242 |
| Application HTTP listener/readiness | Not reached |
| Original application grant/revoke acceptance | Neither of the two required executions started |

The tuple count is reconstructed from the fixed seed and migration source;
request bodies are not logged. The 3,419-byte transport length matches the
reconstructed 40-tuple request. The migration control flow proceeds to its
matrix only after tuple reconciliation. Similarly, reaching the next matrix
request establishes that the preceding successful decision matched the SQL
oracle; the trace itself logs RPC status, not the boolean response body.

### Exact failure boundary

Only original service records before the first `Guest diagnostic:` replay
are counted. There are 249 Check RPCs: one bootstrap Check, followed by
248 matrix attempts. Their starts and ends are balanced.

The final request has `content_length=199`:

| Event | Guest transport timestamp | Host receipt timestamp |
| --- | --- | --- |
| HTTP start | `04:47:37.338289` | `04:48:30.645Z` |
| RPC start | `04:47:37.380149` | `04:48:30.687Z` |
| RPC end | `04:47:40.685625` | `04:48:33.995Z` |
| HTTP end | `04:47:40.824904` | `04:48:34.133Z` |

The RPC reports `Code(4004)`, `context_state="deadline"`, and
`duration_ms=3305`; the HTTP interval is 3,486 ms. The application reports
`OpenFGA returned 500 Internal Server Error; expected 200` and exits with
status 1. The host receipt intervals are 3,308 and 3,488 ms respectively.
Guest and host printed timestamps have different clock origins; their
approximately 53-second offset is not a measured delivery delay.

The new pre-cleanup observation identifies a required-service exit: the
application has exited while OpenFGA PID 151 is still in state `S`. The guest
wrapper records `reason="entrypoint-exited"`, at its own elapsed 457 seconds,
before its unchanged 900-second readiness deadline. This failure is not the
readiness guard expiring. The later diagnostic dump repeats earlier log
lines; those repeats are not additional executions.

### Source-derived failing permission

The fixed seed, `Source::read` ordering and `actions()` arrays reconstruct
the following request at matrix position 248:

```text
user:        user:bob
object:      lakekeeper_table:analytics/requests
relation:    can_get_metadata
consistency: HIGHER_CONSISTENCY
```

This identity is inferred from the source and the uninterrupted request
sequence, not captured from a request body. All 248 reconstructed request
lengths match their observed `Content-Length`, without a mismatch.

There are 207 checks per user, and six users including the synthetic
outsider: 1,242 checks in the full matrix. All 207 Alice checks completed;
Bob's first 40 checks covered `analytics/errors` and `analytics/latency`.
The first Bob check on `analytics/requests` timed out. The remaining checks
were not executed; this is not a claim that they would all fail, or that a
percentage of the whole product is working.

| Source-derived `can_get_metadata` request | Matrix position | RPC result |
| --- | ---: | --- |
| Alice on `analytics/requests` | 41 | `OK`, 196 ms |
| Bob on `analytics/errors` | 208 | `OK`, 63 ms |
| Bob on `analytics/latency` | 228 | `OK`, 2,083 ms |
| Bob on `analytics/requests` | 248 | Deadline, 3,305 ms |

Bob owns `analytics/errors`, giving it a direct ownership path. For both
`analytics/latency` and `analytics/requests`, the expected permission comes
from Bob's nested role membership and the same inherited project grant
through namespace `metrics`, warehouse `analytics` and project `acme`.
Their required allow-path depth is the same. The model therefore identifies
a computed permission to investigate, but it does not establish why the
last request took longer than the preceding table's request.

### Timing and observability limits

Among the 247 successful matrix RPCs, the minimum was 22 ms, median 139 ms
and maximum 2,666 ms. Fifteen took at least one second, including two taking
at least two seconds. The six immediately preceding successful RPCs took
270, 279, 302, 272, 196 and 171 ms. This is variable latency, not an observed
uniform slowdown of all recent requests.

There is no independent evidence identifying either a particular graph
branch, Go scheduling, or an emulator pause as the cause. The 122 QEMU
telemetry records are threshold-triggered module-registration snapshots,
not periodic heartbeats. The last one arrived 77.229 seconds before the
failed RPC; many successful Checks ran after it. Its absence during the
failed RPC says nothing conclusive about execution progress, CPU occupancy,
GC pauses or goroutine waiting during that interval.

## Original PostgreSQL-backed OpenFGA full stack

The second fresh VM ran after the first checker had exited, its evidence
had been retained, and the four original served files had been restored and
verified. Its checker interval was `04:48:54.500` to `04:56:45.434`, or
470,934 host milliseconds. Both VMs used Chromium `157.0.8103.50` on the
same four-logical-CPU runner.

PostgreSQL initialization, both database creations, OpenFGA's actual SQL
migration, its listener, the original bootstrap and the Rust application
process all started successfully. The original 3,419-byte migration Write
returned RPC `OK` in 1,588 ms and HTTP in 1,742 ms. The subsequent tuple Read
completed and execution entered the equivalence matrix.

There are exactly three original Check RPCs before diagnostic replay:

| Request | RPC result |
| --- | --- |
| Bootstrap negative Check | `OK`, 1,246 ms |
| Matrix #1 | `OK`, 2,371 ms |
| Matrix #2 | `Code(4004)`, deadline, 3,685 ms |

The source-derived requests are Alice on
`lakekeeper_table:analytics/errors`: first `can_get_metadata`, then
`can_include_in_list`. Their reconstructed body lengths, 199 and 202 bytes,
match the two observed transport lengths. Actual request bodies are not
present in the trace.

In the fixed model, `can_include_in_list` is a computed reference to
`can_get_metadata`. Alice is expected to be allowed through the parent
namespace's ownership. Thus the failed request is not an observed wrong
authorization decision; it is a deadline before a successful decision was
returned. The preceding request's success does not make the second request
free: the actual Check path still executes.

The failed RPC's guest timestamps are `04:55:34.373115` to
`04:55:38.058430`; its host receipt timestamps are `04:56:25.813Z` to
`04:56:29.497Z`, an interval of 3,684 ms. The guest HTTP interval is
4,013 ms, versus 4,015 ms by host receipt. There is no extra multi-second
log-delivery delay apparent at this boundary. Sparse QEMU telemetry again
does not identify the cause: its last snapshot predates the RPC by 71.682
seconds.

The application reports the same HTTP 500 and exits with status 1.
Pre-cleanup records show app PID 228 exited and OpenFGA PID 167 still in
state `S`. The guest reports `entrypoint-exited` at its elapsed 411 seconds,
before the unchanged 900-second readiness guard. Result fields remain
`ready=false`, `accepted=false`, `checks=0`, `passes=[]`, and
`LAB|boot-failed`, with no page error.

The previous Chromium/two-vCPU experiment failed on this same initial
Write, whereas both new executions completed it. The earlier runner was
different. These observations show that the failure boundary can vary;
they do not prove a particular SQL defect, a speedup or improved reliability.

## Evidence integrity

The first-arm artifact `14602104124`, `pr36-stack-memory-first-arm`, contains
304,752 bytes and has verified SHA-256
`f58ec25de44aed2dd680422aa71a96607a9b56cbd31c89c26373d56436319e7d`.

All five result-file hashes match the original summary. Its 204,926
contiguous source chunks in 1,464 batches reconstruct the exact
242,992-byte console from the 980,186-byte JSONL. The 122 whole QEMU chunks
and all of their 38,113 bytes remain in the original evidence. No sequence
gap, duplicate strict-JSON key, mixed telemetry chunk, incomplete LAB
record or browser page error was found. Capture is well below its
unchanged one-million-chunk and 16-MiB bounds.

Independent review reproduced the transformation using the exact workflow
implementation and pinned source scripts. Its proof and output hashes
match the overlay and the startup manifest observed in the live browser.
The production checker hash and its two declared harness substitutions
also match. The original acceptance implementation is unchanged.

The final paired artifact `14603090975`,
`pr36-full-stack-datastore-comparison`, contains 20,432,058 bytes and has
verified SHA-256
`2a0ce38614395e02da10d91f0c66cc64e6d430a2b61fa97dc4992cdca7c287cd`.
Its memory records match the interim artifact byte-for-byte. The PostgreSQL
arm contains 64,678 contiguous source chunks in 420 batches, reproducing
the exact 96,149-byte console. The raw files match their original summary
hashes. Both captures are complete.

The original source records, prepared baseline, restoration proof, shared
materialization/emulator/topology proofs and mode-specific overlays retain
the input chain. The five original artifact provenance records contain no
recovery fields, and no retained-mirror provenance was recorded in this
execution. The final workflow outcome records both checker outcomes as
`failure`, and `bothCheckerStepsPassed=false`.

The native OpenFGA producer source is
`bb8fed250bdee9931f7702ae2475d28e5e9eed8f`; the separately pinned Rust
bootstrap producer source is `207d15fab685d3adb9c5d51a048982c2236ae75c`.
These are separate from the original `99f00cc` application/rootfs base.
The seventeen-input native verifier covers the OpenFGA/model build scope,
not the entire Rust application. The independent app-source comparison
above and unchanged runtime-file records provide the separate app binding.

Independent local audit rehashed 175 retained emulator members, 22
available original-source members and nine archived diagnostic source
files. The 546,670,592-byte original rootfs is not included in the final
artifact, so this local audit verified its preserved binding records,
not those omitted rootfs bytes anew. Its recorded SHA-256 is
`beb2efc092ed90e5e9919ff4f1f33f7d239aeca19a322025467f7406b09302b5`.

The [retained evidence directory](pr36-stack-evidence) includes original
console and JSONL bytes, results, summaries, both overlays, restoration and
input provenance, plus independently derived analyses. The
[read-only analysis script](pr36-stack-evidence/analyze-stack-evidence.py)
verifies archive hashes, summary-bound files and source reconstruction,
excludes diagnostic replay, and separates bootstrap from application Check
RPCs. It does not infer request subjects or policy relations from log
metadata. Screenshots and original binary producer evidence remain in the
complete Actions artifact. The retained text survives that artifact's
expiry; executable reproduction still requires the original runtime/input
archives or the existing retained-input mirrors.

The workflow uses `continue-on-error: true` to allow the controlled second
arm after a captured functional failure. Consequently, a green individual
Actions step conclusion is not evidence of application acceptance. The
raw child exit status, guest state, and `applicationAcceptancePassed` are
the result authorities.

## What this establishes about the stand

The browser executes the real Linux/OpenFGA/Rust/PostgreSQL stack far enough
to perform substantial application initialization and real computed
authorization. The earlier successful
[isolated component](pr36-memory-component-findings.md) still demonstrates
two direct-ownership deny/grant/allow/revoke/deny cycles. It does not replace
the full application gate.

The application awaits `authorization_migration::migrate` before
`TcpListener::bind` in the pinned
[`src/server.rs`](https://github.com/qqrm/openfga-axum-lab/blob/444b069daf16e9606a0bbf1ea0aea7f4d61b97d8/src/server.rs).
Therefore either observed failure
prevents a ready HTTP API. The two required real application vectors,
`403 -> 204 -> 200 -> 204 -> 403`, have not been demonstrated by either
execution. Authentication, catalog operations and grant/revoke through
Axum cannot be marked browser-validated from these partial startup checks.
Firefox was not executed in this paired workflow. The complete production
browser gate has not passed.

The [browser page](https://github.com/qqrm/openfga-axum-lab/blob/444b069daf16e9606a0bbf1ea0aea7f4d61b97d8/browser-single/pages/index.html)
is a minimal acceptance console with
start, recheck and reset controls. It is not the full interactive catalog
UI; the source page explicitly points to the separate Codespaces/local
Docker lab for that UI. Passing the backend browser gate alone would not
turn this page into the full catalog application.

The memory failure shows that OpenFGA's PostgreSQL datastore is not a
necessary condition for this particular Check timeout. PostgreSQL for the
Rust application's data was still running, so the result does not remove
all database load or isolate every source of contention. Reducing the
migration Write batch is not supported as the fix for this observed case:
the complete batch and reconciliation already succeeded.

The next narrow investigation is the computed Check on the already loaded
graph, preserving all 40 tuples, the original model and deadlines. The
current trace does not justify declaring either an inherent inability to
run OpenFGA in a browser or a proven remedy. The
[one-vCPU results](pr36-vcpu-findings.md) remain unchanged: neither one-vCPU
case passed the full application gate.

One source-supported diagnostic candidate is to vary only the graph
reducer's branch concurrency, `OPENFGA_RESOLVE_NODE_BREADTH_LIMIT`, from its
default 10 to 2 while keeping two guest CPUs. The pinned upstream
[configuration](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/pkg/server/config/config.go)
and [environment binding](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/cmd/run/flags.go)
support that parameter without rebuilding the executable. This is a
per-reducer concurrency bound, not a vCPU count or a global goroutine cap.
It is an unrun hypothesis, not an implemented fix; lower concurrency may
also delay a positive branch. A shortened fixture that fails to reproduce
the original control cannot establish that tuning fixes the full workload.

The matrix reconstruction uses the pinned
[`authorization_migration.rs`](https://github.com/qqrm/openfga-axum-lab/blob/444b069daf16e9606a0bbf1ea0aea7f4d61b97d8/src/authorization_migration.rs)
and [model](https://github.com/qqrm/openfga-axum-lab/blob/444b069daf16e9606a0bbf1ea0aea7f4d61b97d8/fga/model.json).
The retained `.10` adaptation reuses schema/TypeSystem and type reachability;
it does not replace real user/tuple authorization with a precomputed allow.

No PR merge, production runtime change or Pages deployment is part of
this diagnostic assessment.
