# Browser OpenFGA startup review

Status of this review on 13 November 2026: **approved changes under validation;
browser acceptance has not passed**. The latest completed browser gate below is
commit `5d18ed2fda49fe84a9ac03c7b8571940f0891819`, run `43388600974`.
The Browser preview run for the current PR head is the authority for later
acceptance results. The initial proposal was prepared against PR #36 head
`c527f3ca016b317350a29c0dca38c07e7699230b`. The user approved publishing changes
and additional checks. The PR has not been merged and Pages has not been
published.

## Current retained result and a local PostgreSQL socket

[Run 43388600974](https://github.com/qqrm/openfga-axum-lab/actions/runs/43388600974)
still completed **zero authorization acceptance passes in each browser**.
Both complete captured streams were reconstructed exactly through their recorded
cutoffs in [reader 43396299271](https://github.com/qqrm/openfga-axum-lab/actions/runs/43396299271).
[Reader 43397679133](https://github.com/qqrm/openfga-axum-lab/actions/runs/43397679133)
then emitted both full consoles; their decoded bytes match the retained SHA-256s.

Chromium booted both guest CPUs and PostgreSQL. OpenFGA warmup (24740 ms),
self-dial (1030 ms), Health and CreateStore succeeded. Its real model decoder
reported `cache_hit=true`, 52708 bytes and 1240 ms. The interval from model HTTP
start to the unary observer was 2750 ms. The RPC then ended after 2310 ms with
code 2058 (`cancelled`) and a canceled context; HTTP ended after 5230 ms.
PostgreSQL identified the canceled statement as the genuine six-parameter
`INSERT INTO authorization_model`. Bootstrap failed closed and Axum acceptance
did not start. The decoder improvement is exercised but insufficient for the
whole request; the later TypeSystem-resolver transfer remains excluded.

Firefox 159.0 reached neither `LAB|boot` nor the services before its 1200000 ms
host limit. The kernel had brought both CPUs online. From devtmpfs initialization
onward, console timestamps stayed at `3.770000`; the last line reports the
virtio block device. Both page-error lists are empty. The last Firefox module
snapshot has 1024 registrations on core 0 and no detachments/GC, well below its
8192 limit. These observations do not identify a cap or memory-barrier cause.

This commit changes **only OpenFGA's PostgreSQL connection host**:
`&host=/tmp/browser-lab` selects the already listening Unix socket. The existing
pgx default `cache_statement`, binary protocol, role, database, SQL statements
and PostgreSQL durability settings remain. Rust's `DATABASE_URL`, the model,
HTTP/gRPC paths and all authorization/request deadlines are unchanged. This
avoids sending local database traffic through the emulated TCP/IP stack; the
resulting WASM latency is still to be measured.

[Native compatibility run 43401420833](https://github.com/qqrm/openfga-axum-lab/actions/runs/43401420833)
passed two consecutive original `lab-tool acceptance` executions:
`403 → 204 → 200 → 204 → 403`. It used the actual browser OpenFGA `.7`
executable, 63054008 bytes, SHA-256
`57ef124e5b339dcf1c3dbcbe2c3083c03f5db74a799a1472014e875292cc1066`.
The native image was freshly built; it is not claimed byte-identical to the
retained guest image. SQL proved role `authzlab`, the existing local trust rule
and ten OpenFGA client connections, all through Unix sockets. Its independent
SQL probe used the other database, so it could not satisfy that pool assertion.
This verifies real-service compatibility; it is not browser performance proof.

The isolated [Firefox boot comparison 43401457205](https://github.com/qqrm/openfga-axum-lab/actions/runs/43401457205)
used both retained emulator pairs with one common current launcher and identical
checked kernel, data and arguments. All four fresh Firefox 159.0 observations
reached `LAB|boot`: old 59549 ms, new 61942 ms, new 60836 ms, old 58385 ms.
Each capture was complete and each page-error list empty. These are host elapsed
times including browser startup and loading, not guest CPU or service timings.
The previous early stall did not reproduce in this one controlled run; this
neither establishes its cause nor excludes an intermittent failure. No kernel
flag or memory-barrier change is justified by this result. This comparison
tested kernel boot only. The required two consecutive authorization passes in
EACH Chromium and Firefox remain the publication gate.

## Previous retained result: model decoding before the RPC

[Run 43378543550](https://github.com/qqrm/openfga-axum-lab/actions/runs/43378543550)
started PostgreSQL and OpenFGA in both browsers. One-cycle warmup, blocking
self-dial, Health and CreateStore succeeded. The initial 52708-byte model POST
then exceeded bootstrap's unchanged five-second operation limit. **Chromium
and Firefox each completed zero acceptance passes**; Axum acceptance did not
begin, and Pages deployment was skipped. All six other workflows passed on
this head, including native Delivery on its first attempt.

| Guest logger measurement | Chromium 158.0.8127.18 | Firefox 159.0 |
| --- | ---: | ---: |
| Warmup duration | 22070 ms | 45370 ms |
| Self-dial duration | 710 ms | 1630 ms |
| HTTP start to unary RPC observer start | 4780 ms | 6450 ms |
| Model HTTP duration | 5380 ms | 5340 ms |
| Model RPC duration | 1140 ms | 990 ms |
| RPC code / context state | 2056 / canceled | 2056 / canceled |

Firefox entered the RPC observer 1110 ms after the HTTP handler ended. The
interval before the observer includes JSON decoding, protobuf/gRPC transport
and scheduling. Generated request validation and authentication run after
that observer; they do not explain this earlier interval. Warmup's separate
JSON decode stage took 3970 ms in Chromium and 7440 ms in Firefox. These are
separate operations, not a profile that attributes the whole HTTP interval to
JSON. The canceled context and code 2056 do not demonstrate an invalid model.
The separately prepared TypeSystem transfer targets a later resolver path
and is not included.

The full capture remained complete: Chromium retained 28805 original chunks
in 9368 batches (69304 bytes); Firefox retained 32793 chunks in 8970 batches
(392679 bytes). Source sequences and exact UTF-8 reconstruction match the
saved consoles. Actual QEMU admission limits were 15000 and 8192 respectively.
No page errors, Module OOM or instantiate-throw events were observed in this
run. This is not a general memory guarantee or a controlled comparison with
previous runs. Guest timestamps, not host delivery times, produced the table.

[Evidence artifact 14017024263](https://github.com/qqrm/openfga-axum-lab/actions/runs/43378543550/artifacts/14017024263)
contains 260772146 bytes, with SHA-256
`f6ec83656a1d4b042623e60fdb2f8f9dd7f1f5e4abc6f0c06d5fca6370538317`.
Both observed startup manifests exactly match the retained build manifest.

## Published decoder correction and native verification

OpenFGA browser build `.7` prepares a private decoder entry during the existing
successful warmup. The standard streaming JSON parser still reads every
request; only the exact pinned raw model can reuse an independent protobuf
copy. The URL supplies the actual store ID and the real gRPC, generated
validation, authentication, model ID generation and PostgreSQL write remain
mandatory. Warmup now calls `ValidateAll`, matching the real interceptor.

The complete build from the pinned OpenFGA source passed the startup/transport
suite (25 top-level tests, 101 including subtests, 68.477 seconds), schema cache
suite (10/20) and command/server regression suites (44/353). The separate
codec/gateway race run passed. The retained Rust `lab-tool` actually sent the
52708-byte key and completed bootstrap against the real memory-backed OpenFGA
gateway; this is not a PostgreSQL or browser acceptance result. Executable
SHA-256: `57ef124e5b339dcf1c3dbcbe2c3083c03f5db74a799a1472014e875292cc1066`.

A separate QEMU correction replaces the compiled `INDEX_op_mb` no-op with
WebAssembly `atomic.fence`, matching the interpreter's full-barrier behavior.
The actual extracted C emits the correct fence for all 64 tested masks; the
old path emits none. Generated Wasm preserves stack and shared-memory behavior
and rejects malformed fence encodings. Tiny emitted-Wasm fixtures also passed
in actual Chromium 157 and Firefox 159. These checks establish the emitted
instruction, not inter-thread ordering of the complete emulator or backend
acceptance. Full fences can add execution cost; no startup-speed claim is made.

The launcher also rejects a browser CPU report below two, or an invalid count,
before downloading the VM. This backend sizes per-CPU vectors from that report,
so its two-vCPU configuration requires at least two slots. The accepted count
is retained in VM evidence. This is a precondition check, not a claim that
inconsistent reports caused the observed browser failures.

The original authorization model, Rust application, bootstrap and acceptance
programs are unchanged. The two/three/five-second request deadlines, startup
budget, emulator instance budgets and both required consecutive passes remain
unchanged. The next full QEMU build and Chromium/Firefox acceptance are still
required before any merge or Pages publication.

## Preceding capture and Firefox admission correction

[Run 43373341199](https://github.com/qqrm/openfga-axum-lab/actions/runs/43373341199)
passed all build/preparation steps, then failed real browser acceptance.
Neither browser recorded a pass; Pages deployment was skipped.
The evidence ZIP SHA-256 is
`fb8258c051e671dd6caa84b8883671d7262831c8e8349c6b9ff089165bbfeeb6`.

| Engine | Retained observation |
| --- | --- |
| Chromium 158.0.8127.18 | The diagnostic transport overflowed its 4096 pending chunks. Of 10567 retained chunks, 10558 contained one character; one serial Playwright binding call per character could not drain the stream. This is a capture failure, not a demonstrated backend failure. |
| Firefox 159.0 | Full capture: 18768 chunks, 79419 bytes, exact sequence cutoff. Linux booted with both CPUs available, then `new WebAssembly.Module` threw `out of memory` during PostgreSQL initialization. No readiness or acceptance. |

The next correction sends original log chunks in bounded batches, preserving
their source sequence and exact text. A compact JSONL record stores each batch's
first sequence, receipt timestamp and original text array. The single pump
avoids a Promise/binding round trip for every UART character. Both pending and
host storage remain bounded; lost chunks, overflow and delivery failures still
prohibit success. The synchronous cutoff captures guest state before draining,
so an error during delivery cannot replace that state with an uninformative
capture error. Nothing in log capture creates or interprets authorization
responses. A real-browser character-burst replay is required in addition to
unit checks; the preceding three-large-chunk test did not cover this failure.

At Firefox's exact module throw, core 1 recorded 94911 registered, 93184 detached
and 86016 finalized instances: 1727 retained and 7168 detached without delivered
finalization. Early detachment and GC callbacks were active. Its 8895 registered
instances without delivered finalization alone exceed 8192; a global cap of
8192 would not admit that compilation under the same prior history. The latest
other-core snapshot is asynchronous and is not added as an exact global total.
Encoded module bytes are not native-memory measurements.

The candidate therefore selects 8192 for Firefox and retains 15000 for
Chromium. One exported C setter configures shared state in main-thread `preRun`;
an atomic freeze at the first backend initialization rejects subsequent changes.
Both admission and the existing budget-induced yield condition read that same
limit. Retention stays 2048 per CPU; CAS reservation, actual callback accounting,
TCI fallback and compilation/yield thresholds are preserved. Earlier Firefox
8192 runs reached OpenFGA with the former four-cycle warmup, while the current
one-cycle/cache combination now reaches OpenFGA in the latest run above but
still fails model initialization. Earlier Chromium evidence
showed long lower-cap admission stalls, so its cap is unchanged. This is an
instance-count candidate, not a guaranteed safe byte budget or a proven fix.

The preceding [run 43364708449](https://github.com/qqrm/openfga-axum-lab/actions/runs/43364708449)
timed out in both browsers and retained only the UI tail. Firefox's early service
logs had been overwritten; that run did not establish a specific service error.
The launcher also replaces the existing kernel `loglevel=0` token in place with
`loglevel=7`, before `--`, and records that level in VM provenance. This exposes
ordinary boot messages that were suppressed after early parameter parsing.
The last KASLR line is emitted through early printk, after the entropy mix;
it does not identify a stall inside that calculation. This is a diagnostic
configuration change, not a claim to repair the kernel startup failure.

A separate upstream instruction defect is now reproduced: the Wasm emitters
for 32-bit signed/unsigned widening multiplication read raw i64 register values.
In particular, signed `-1 * 2` with clean upper bits produces an incorrect high
half. The correction normalizes each operand to signed/unsigned i32 before the
i64 multiplication: exactly eight additional emitter calls. Tests execute
bytes emitted by extracted actual C functions; the original fails 42 of 96
cases, the correction passes all 96. This does not attribute the browser
timeouts to that defect or establish browser acceptance. Source context:
[upstream PR 54](https://github.com/ktock/qemu-wasm/pull/54),
[pinned emitter](https://github.com/ktock/qemu-wasm/blob/8604ed49a3cde392890b014a8d5a959c8a2fe72a/tcg/wasm32/tcg-target.c.inc).

The actual corrected emulator passed the emitted-Wasm regression checks in CI
but still encountered the Firefox OOM above. The OpenFGA `.6` build and request
deadlines remain unchanged. A separately tested one-shot TypeSystem transfer
has not been included: the latest run provides no captured model-resolution
deadline to justify it. Ordinary CI and the native single-image checks passed;
the separate native Delivery job failed readiness recovery after restarting its
official OpenFGA container. No service logs were retained for that failure, so
its cause is unproven. One unchanged failed-job rerun passed all Delivery steps;
this establishes that the failure did not reproduce in that retry, not its cause.

## Diagnosis

The pinned OpenFGA 1.10.3 HTTP gateway establishes a blocking connection to
its own gRPC listener using a hard-coded three-second startup timeout.
`OPENFGA_REQUEST_TIMEOUT` and `OPENFGA_HTTP_UPSTREAM_TIMEOUT` govern requests,
not that self-dial. Disabling HTTP avoids the connection but also removes the
interface required by the existing bootstrap and HTTP-only Axum build; it is
not a configuration-only repair of this runtime.

Primary sources: [upstream run.go](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/cmd/run/run.go),
[upstream configuration](https://github.com/openfga/openfga/blob/293893b22a53d8a130a5f7c97cab4bc8e91bcaa4/pkg/server/config/config.go).

PostgreSQL startup was already ordered correctly: `initdb`, `pg_ctl -w`,
database creation, and OpenFGA schema migration precede `openfga run`.
The original entrypoint then immediately starts `lab-tool bootstrap`, whose
60-second readiness loop polls HTTP every 250 milliseconds. Under emulation,
this can compete with OpenFGA startup and spend the bootstrap budget before
the HTTP listener is available.

The Pages runtime restores the older, checksummed `99f00cc` application
filesystem. Editing the repository's entrypoint alone does not update that
filesystem. The launcher must explicitly supply the reviewed startup files
and adapted executable to the guest.

## Proposed change

- Build OpenFGA from the exact upstream commit with Go 1.24.6. Change
  the gateway startup self-dial budget from three to 60 seconds, observing
  parent cancellation. Preserve `grpc.WithBlock()`, TLS options and fatal
  startup failure. Record source, patch, compiler and executable provenance.
- Before opening listeners, exercise real codecs and fully validate the exact
  pinned model once under one 60-second startup budget. Retain only the proven
  validity of its immutable conditionless schema body. Matching real model
  writes reuse this validation and still write PostgreSQL; resolver loads create
  a fresh TypeSystem with the real ID. Full protobuf comparison excludes only
  the generated model ID; every other model uses the original validator.
  No datastore state, graph, TypeSystem or authorization decision is cached by
  this adaptation. Record stage timings. Omit unary raw-payload logging work
  while retaining compact transport metadata and the security middleware.
- Reuse JSON-to-protobuf decoding only for the exact pinned raw model body,
  preparing a private prototype within that same successful startup phase.
  Keep the standard streaming JSON parser, copy the protobuf independently for
  each matching request, and retain default decoding for every other input.
  The generated gateway supplies the real URL store ID; gRPC, validation,
  authentication, deadlines and the actual model write still run. This codec
  change targets the latest pre-observer delay, not resolver graph construction.
- Supply the adapted executable and current entrypoint through the existing
  host mount, with checksums verified before VM initialization. Use the
  converter's `mr` instruction for a read-only bind mount. Its `m`
  instruction is writable; see the pinned
  [mount parser](https://github.com/container2wasm/container2wasm/blob/087c313479d51cdf255e4f28ded6ae7d8141297b/cmd/init/main.go).
- Wait for OpenFGA's loopback HTTP listener with a bounded wait and process
  liveness checks, then invoke the original bootstrap once. A listening
  socket only schedules bootstrap: the real `SERVING` response, exact model
  binding and subsequent operations still decide readiness.
- Require a distinct start/result pair for each acceptance pass, retain both
  results, and clear current readiness/acceptance on guest failure. Check
  PostgreSQL and the service supervisor while awaiting another command.
- Keep Pages publication behind a separate manual request, after the actual
  browser gate succeeds.
- Use two sockets with one CPU each, and verify both CPUs online inside Linux.
  Rebuild only the QEMU JS/WASM pair with earlier compiled-module eviction.
  Preserve the original data/rootfs, loader and reviewed kernel bytes.

The PostgreSQL and Axum binaries, `lab-tool acceptance` and the pinned Lakekeeper
model stay unchanged. `initdb` receives the existing runtime connection/memory
budget; this does not skip its real IPC probes or change durability. No
response is synthesized. Existing request deadlines remain unchanged:
two seconds for Axum's OpenFGA client, five seconds for bootstrap operations
and the acceptance HTTP client, and the original OpenFGA request deadlines.

## Observed verification and its limits

The local environment rolled back during a later diagnostic attempt. The
following records summarize tool output observed before that rollback;
the compiled executable and raw local browser logs were lost. These are
observations, not a substitute for retained CI evidence or a new passing run.

| Check | Observed result before rollback |
| --- | --- |
| Native OpenFGA regression package | Passed in 67.187 seconds. A real delayed gRPC handshake fails with the upstream three-second limit and succeeds with the adaptation; an unavailable server still fails at the actual 60-second limit. Cancellation and unchanged authorization defaults were checked. |
| JavaScript protocol and payload tests | Nine tests passed, including duplicate/stale result rejection and corrupt or missing startup payload rejection. |
| Static validation | Shell syntax, ShellCheck, actionlint and whitespace checks passed. |
| Chromium 157.0.8103.78, one virtual CPU | Linux and PostgreSQL started. The adapted OpenFGA reached its HTTP listener. Bootstrap subsequently failed with `Error: Elapsed(())`; no acceptance sequence completed. |
| Firefox | Skipped after Chromium failed; no Firefox acceptance result. |
| Two-CPU diagnostic | Interrupted by the environment rollback. Outcome unknown; not a passing test. |

The Chromium result establishes HTTP startup with the adapted executable,
not successful integration or proof that the original three-second limit caused
the earlier Chromium failure. The bootstrap's outer readiness timeout is mapped
to `OpenFGA unavailable`; the unwrapped `Elapsed(())` therefore points to a
later per-operation timeout. At that point the exact operation was unknown;
the retained attempt below identifies it. Do not increase authorization or
operation deadlines to conceal this remaining limit.

After recovery, a new build repeated all native regression tests successfully
in 67.234 seconds and produced the identical executable SHA-256
`f10495a3aeea59270be91d7017749618cc04eedc0b934682fb58d24cc85908d9`.
The raw test log is emitted with the build artifacts. This confirms the
native startup adaptation again; it does not establish browser acceptance.

The first locally tested patched wrapper was silent throughout service startup. Its
900-second readiness budget also began after copying the adapted executable,
so that copy was outside the guest budget. Phase markers and an overall
startup bound beginning before payload preparation are included in the restored
proposal to distinguish copying, database initialization, migration, bootstrap
and Axum startup. Subsequent real CI runs used this diagnostic wrapper, as
recorded below.

## Retained browser evidence on 12 November 2026

- [Run 43297886288](https://github.com/qqrm/openfga-axum-lab/actions/runs/43297886288)
  started PostgreSQL and OpenFGA in Chromium, then bootstrap returned
  `Elapsed(())`. No acceptance sequence completed.
- [Run 43301700456](https://github.com/qqrm/openfga-axum-lab/actions/runs/43301700456)
  enabled upstream INFO diagnostics. A real CreateStore RPC completed, but
  the HTTP bootstrap still timed out. Its completion log alone did not show
  which client operation failed.
- [Run 43306901697, attempt 1](https://github.com/qqrm/openfga-axum-lab/actions/runs/43306901697/attempts/1)
  exhausted the 900-second startup budget while `initdb` was selecting
  `max_connections`, before OpenFGA ran. The cause of this earlier stop is
  unknown. It is not a failed execution of the new OpenFGA transport observer.
- [The unchanged attempt 2](https://github.com/qqrm/openfga-axum-lab/actions/runs/43306901697/attempts/2)
  completed PostgreSQL initialization and localized the model-write timeout.
  Chromium completed zero acceptance sequences; Firefox was skipped by the
  then-current checker. Pages was not published.

The last attempt used Chromium 158.0.8127.18, two configured guest CPUs and
512 MiB. The source runtime ZIP, derived data, kernel and entrypoint are
identical between the two attempts. Their build/startup manifests also match.

| Actual operation | HTTP duration | Unary RPC duration | Result |
| --- | ---: | ---: | --- |
| Health | 1580 ms | 830 ms | `OK`; bootstrap proceeded |
| CreateStore | 2650 ms | 1650 ms | `OK`; bootstrap sent the model |
| WriteAuthorizationModel | 5600 ms | 11410 ms | HTTP cancelled before server unary handler began; RPC later returned OpenFGA code 2058 (`cancelled`) |

The 52708-byte model request began at 21:46:24.150 UTC. Its HTTP handler
returned with a cancelled context at 21:46:29.780; the server unary handler
started at 21:46:30.450 and returned at 21:46:41.860. The approximately
6.30-second interval before the unary handler includes HTTP body decoding,
gRPC codecs/transport and scheduling. The later 11.41-second interval also
includes the upstream logger's eager payload serialization. Neither interval
is a profile of a single function. The repeated lines in the failure console
are a replay of the log tail, not a second bootstrap or acceptance pass.

[Evidence artifact 13986943099](https://github.com/qqrm/openfga-axum-lab/actions/runs/43306901697/artifacts/13986943099)
retains the console, browser results and build records. Its ZIP SHA-256 is
`df74002ce935ca43ac4a0f109d33e81b011b270fe9a4bd9a28300f0948f7ae30`.

These observations motivate the startup codec/validation warmup and removal
of unary raw-payload logging. They do not prove that either change is enough.
Native profiling of the exact model found repeated parsing and graph-building
work even after warming; it cannot predict the effect of QEMU's translated-code
cache. The candidate must still pass the real browser gate with unchanged
request deadlines. The current checker attempts Firefox even if Chromium
fails, so both engines can provide independent evidence.

## Latest real browser failure and the next bounded candidate

[Run 43318362131](https://github.com/qqrm/openfga-axum-lab/actions/runs/43318362131)
tested `077452de` in both browsers independently. It completed **zero acceptance
passes in Chromium 158.0.8127.18 and zero in Firefox 159.0**. The native build,
source/runtime verification and other workflows passed; deployment was skipped.

In Chromium, the exact-model warmup completed in 25480 ms and the startup
self-dial in 1050 ms. Health and CreateStore succeeded. The real model POST
spent 2080 ms before entering the unary handler, then 4870 ms inside it. The
HTTP client timed out and the RPC returned OpenFGA code 4004
(`deadline_exceeded`). The self-dial was therefore not the failing phase in
this run, and increasing only that startup budget did not establish acceptance.

Firefox loaded Linux and staged the executable, then failed during `initdb`,
before OpenFGA started. Its error was `out of memory` at
`new WebAssembly.Module(wasmBytes)` inside a QEMU CPU worker. The previous
checker recorded this error but waited for its 20-minute polling timeout.
This identifies browser-side compilation/allocation failure; it does not prove
that Linux exhausted its 512 MiB or identify the exact exhausted allocator.

[Evidence artifact 13991471710](https://github.com/qqrm/openfga-axum-lab/actions/runs/43318362131/artifacts/13991471710)
retains both consoles, results and build records. ZIP SHA-256:
`a700be01b871788666888a0ab4cdf5706178ab3a1a1dde60afdaafcd3069e3aa`.

The Chromium failure snapshot also reported `/sys/devices/system/cpu/online`
as `0`, despite two requested CPUs. With ACPI disabled, this kernel uses the
legacy MP table. Pinned SeaBIOS lists the first logical CPU per package; the
previous `2,sockets=1` topology exposed only one CPU through that table.
A short actual Chromium 157/QEMU WASM kernel A/B, using the same firmware and
kernel, confirmed one CPU for that topology and two for
`2,sockets=2,cores=1,threads=1`. The guest now checks `online=0-1` before services.
Sources: [QEMU CPUID topology](https://github.com/ktock/qemu-wasm/blob/8604ed49a3cde392890b014a8d5a959c8a2fe72a/target/i386/cpu.c)
and [SeaBIOS MP table](https://github.com/coreboot/seabios/blob/a6ed6b701f0a57db0569ab98b0661c12a6ec3ff8/src/fw/mptable.c).

A subsequent local full Chromium 157 run with both CPUs online still failed
the model write: warmup 25310 ms, self-dial 1330 ms, model pre-handler interval
4740 ms and unary interval 11200 ms, ending with cancelled code 2058. No
acceptance pass completed. Different browsers/hosts and changed configuration
prevent treating these CI/local timings as a controlled performance comparison.

A separate local Firefox 159 diagnostic preserved the guest and request limits,
adding numeric counters around dynamic module creation/GC in the served emulator
JS. It reproduced OOM creating a 22947-byte module. Core 0 had registered 6250
instances; core 1's latest snapshot reported 4864, for a lower bound of 11114.
Neither core had detached an instance or reported a GC callback. The original
15000-instance eviction threshold had not been reached. These are per-core
counters, not measurements of native-code bytes or RSS. The production error
propagation/checker changes correctly retained `accepted=false`, recorded the
runtime error and ended this diagnostic without waiting 20 minutes.

The first [emulator candidate](../browser-single/qemu/README.md) changed only
`MAX_INSTANCE_ALIVE` from 15000 to 4096. It uses the existing eviction, GC and
TCI fallback; the compilation threshold remains 1500. `tb-size` is a different
buffer and its flush does not release those compiled-module references.
4096 is a measured-motivation candidate, not a proven safe byte budget or a
performance result. Its subsequent actual build passed, but its browser gate
failed as recorded below.

The earlier OpenFGA candidate repeated fresh codec/validation work four times under the
same total 60-second startup budget. Neither TypeSystem nor validation results
were cached by that revision.
That revision left every real request unchanged. Increasing administrative model RPC budgets
was rejected as a shortcut: it would require separating authentication/access
control deadlines, and the first real Check/tuple Write would still load and
validate the model through the server resolver cache with its original deadline.

Before publishing this candidate, a fresh native build from the pinned upstream
commit passed the targeted startup, transport and four-cycle warmup tests in
67.495 seconds. Its executable SHA-256 is
`2c489ec771fb58ba657bd0dce687fd0416605c13075e66a43fed7d58dc3f77fc`.
All 13 protocol/payload/runtime-error JavaScript tests passed, as did shell
syntax, ShellCheck 0.10.0/0.11.0 and whitespace checks. File-integrity fixtures
verified a paired emulator overlay and rejected corrupted JS or an altered
upstream build recipe before replacement. These checks are not an emulator
build or browser acceptance. The actual run below supersedes the pending status.

## Actual 4096-instance candidate and deferred GC

[Browser preview 43330935595](https://github.com/qqrm/openfga-axum-lab/actions/runs/43330935595)
tested commit `6e6a539c` and failed with **zero passes in both browsers**.
Both executable builds, runtime integrity checks and the unchanged Lakekeeper
demo passed. Pages upload and deployment were skipped.

| Browser | Last confirmed phase and result |
| --- | --- |
| Chromium 158.0.8127.18 | Linux and both CPUs started. The last guest marker was PostgreSQL initialization. The host's 20-minute wait expired; no OpenFGA phase was recorded. |
| Firefox 159.0 | PostgreSQL became ready and OpenFGA migration completed. Warmup cycle 1 completed in 38750 ms; the shared 60-second budget expired in cycle 2 TypeSystem validation, before listeners or self-dial. No page error/OOM was recorded. |

The Firefox cycle 1 stages were JSON 7380 ms, protobuf 2450 ms, generated
validation 610 ms, TypeSystem 18690 ms and response codec 9620 ms. Cycle 2
recorded 8830, 1710 and 630 ms in its first three stages, then 9780 ms elapsed
in unfinished TypeSystem validation. Cycles 3 and 4 did not start. Failure-tail
replays in the console are not additional warmup cycles.

[Evidence artifact 13998636550](https://github.com/qqrm/openfga-axum-lab/actions/runs/43330935595/artifacts/13998636550)
has ZIP SHA-256 `e1cc8a97f5f46e16c78a17e8b5688e6e91f46f132754199552e4102d03692dae`.
The actual 4096 emulator WASM hash is
`68c60a823df0c90a5dc054c45c140da5d63ce48381e0b9c769084aafcc6dce11`.

Two local Chromium 157 diagnostics preserved that WASM, the guest and deadlines,
adding numeric JS counters only. At 4096 registered instances, about half had
been detached, but no GC callbacks had arrived and new compilations stopped.
Callbacks later resumed compilation. In the first probe, observed waits were
about 66 and 105 seconds. The second probe also counted the actual Asyncify
`emscripten_sleep(0)` callback: hundreds occurred before the first observed GC
callback, about 30 seconds after detachment. Samples were five seconds apart;
instrumentation may affect timing. This is evidence of delayed collection with
frequent yielding, not a permanent deadlock or a production performance result.
The second probe console SHA-256 is
`4750f95eda155b6b30a57fade01d98fc71549adece40fc247e7541b2c1b5b23c`.

The revised QEMU patch separates early FIFO detachment at 2048 retained
instances per CPU from a strict global 8192 live-instance limit. It reserves
slots with atomic compare/exchange and keeps detached instances counted until
GC callbacks are processed. Repeated eviction is allowed with pending GC only
after the retained threshold has been reached again; it cannot repeatedly
halve a below-threshold cache while blocked. `INSTANTIATE_NUM=1500`, the yield
counter and real application deadlines remain unchanged. The thresholds still
need real browser validation; 8192 instances is not a guaranteed safe byte
budget. This candidate is not evidence of successful acceptance.

Before publishing the revision, native tests of the extracted accounting and
eviction functions passed all five scenarios, including a deterministic
two-thread race for the last slot. Both negative controls rejected the old
pending-GC guard and non-atomic admission. The exact C postimage is Git blob
`207a59b01ee8655fb542173fd944549653c3d29c`. Patch application/reversal, both
ShellCheck versions, Python/JavaScript syntax and the 13 existing browser
protocol/payload/error tests passed. Native adapters do not test browser GC,
pointer-width-dependent TB lookup or actual WASM execution; CI must do that.

## Actual 8192-instance gate and exact-schema cache candidate

[Browser preview 43343577131](https://github.com/qqrm/openfga-axum-lab/actions/runs/43343577131)
tested `23562aca` and failed with zero passes in both browsers. PostgreSQL and
OpenFGA migrations completed in both. All six other workflows passed; Pages
upload and deployment were skipped.

| Browser | Observed failure |
| --- | --- |
| Chromium 158.0.8127.18 | Four warmup cycles completed in 53000 ms; self-dial succeeded in 550 ms. Real health and CreateStore succeeded. The model POST reached its unary handler after 2800 ms, exceeded the original five-second client budget and ended cancelled; its handler lasted 7110 ms. Axum did not start. |
| Firefox 159.0 | The shared startup budget expired in the third cycle's response codec after 60300 ms. No listeners, self-dial, bootstrap or acceptance were reached. |

No page error/OOM was recorded. The evidence ZIP is
[artifact 14002636348](https://github.com/qqrm/openfga-axum-lab/actions/runs/43343577131/artifacts/14002636348),
SHA-256 `81fd4e84dbd5c8e6e0fded9767842c6be3f00f3e8d368424e8d8143a12aadbf9`.

A subsequent local Chromium 157 numeric diagnostic used this exact 8192/2048
WASM and unchanged guest. Throughout the observed OpenFGA warmup, both cores'
registration/collection totals remained unchanged at the full live budget:
23552 registered minus 15360 collected = 8192; 5120 were detached pending GC
and 3072 remained retained. Thousands of actual zero-sleep callbacks occurred.
The shared warmup deadline expired in the fourth cycle's TypeSystem stage at
60060 ms, with zero acceptance passes and no page errors. This directly links
that diagnostic's warmup to blocked JIT admission; it does not prove identical
counter state in the uninstrumented CI run. Console-window resets replay old
lines, so replayed events must be deduplicated when interpreting counters.

The next OpenFGA adaptation uses one full startup validation and memoizes only
the validity of the exact conditionless protobuf schema body. This avoids the
write-model command's otherwise discarded TypeSystem and repeated schema
validation in resolver loads. It never shares a TypeSystem or graph: resolver
loads still call `New` on their real datastore model. Every body mismatch uses
the complete original validator, including conditional models. Deep cloning,
unknown-field comparison, independent returned state, cancellation and normal
fallback require focused regression tests. Successful native checks do not
establish that remaining graph/JIT costs meet the real browser deadlines.

The full native candidate build passed the startup/transport/warmup package in
67.317 seconds and the ten focused schema-cache tests in 0.601 seconds. A separate
isolated run passed all 42 top-level TypeSystem tests (396 including subtests),
and focused `-race` testing passed. The executable SHA-256 is
`98e2eabee1ee8861c6e672c043547e7742278bef56cb17254b559fa91df17391`.
Normal runtime preparation verified this executable and the unchanged emulator,
data, loader, original arguments and kernel. These are native/build checks;
real browser acceptance for that candidate was still pending at build time.

A local Chromium 157 run of this exact binary and the unchanged 8192/2048
emulator then failed with zero passes. Its single warmup completed in 21180 ms
and self-dial in 450 ms. Real health/CreateStore succeeded, but the model POST
took 4400 ms before its unary handler and exceeded the original five-second
operation limit. HTTP ended cancelled after 5180 ms; the unary handler ended
after 2140 ms with code 2056. Axum did not start. There is no claim that the model
write committed. No browser page error was recorded. This proves that the
schema optimization alone is insufficient on that runtime; different runs are
not a controlled latency comparison.

The accompanying emulator candidate therefore restores the original upstream
15000 live-instance backstop, while preserving early 2048-per-CPU detachment,
strict CAS reservation, real GC accounting and compilation/yield thresholds.
The new C postimage is `8075f1b4ee8569dfda0934d02268d755d5151ef8` and differs
from the 8192 C postimage by one numerical line. All five native accounting
scenarios and both negative controls passed at the new bound, including
14999-to-15000 concurrent admission; patch application/reversal passed.
This retains a finite count limit and adds allocation headroom after detachment.
It still cannot guarantee GC scheduling or a safe byte budget. The combined
candidate needs a new actual emulator build and two real passes per browser.

## Allocator-debug performance candidate

The original QEMU command line enables `slub_debug=F` for all kernel slab
caches. Linux documents its performance cost; the implementation moves
debug caches out of the allocation fast path and disables lockless freelists.
The pinned kernel configuration has `CONFIG_SLUB_DEBUG_ON` disabled.
Removing that explicit debugging token is a candidate that preserves the
application's deadlines, authorization behavior and PostgreSQL durability.
Its effect on this workload has not been measured.

The launcher removes that allocator-debug token, with assertions against the
reviewed original arguments. It keeps 512 MiB guest memory and all request
deadlines. The original attempt to request two CPUs did not expose both to Linux;
the later topology finding and actual A/B above correct that claim. The combined
candidate still requires real browser acceptance.

Primary sources: [SLUB documentation](https://github.com/torvalds/linux/blob/830b3c68c1fb1e9176028d02ef86f3cf76aa2476/Documentation/mm/slub.rst),
[allocator implementation](https://github.com/torvalds/linux/blob/830b3c68c1fb1e9176028d02ef86f3cf76aa2476/mm/slub.c),
[converter kernel configuration](https://github.com/container2wasm/container2wasm/blob/087c313479d51cdf255e4f28ded6ae7d8141297b/config/qemu/linux_x86_config).

Do not disable asynchronous Go preemption as a latency fix: the pinned Go
runtime explicitly warns that this can delay scheduling and garbage
collection. Its default `GOMAXPROCS` already follows the detected virtual CPU
count; raising it without providing another CPU does not supply parallel
execution. See [Go runtime controls](https://github.com/golang/go/blob/go1.24.6/src/runtime/extern.go)
and [scheduler initialization](https://github.com/golang/go/blob/go1.24.6/src/runtime/proc.go).

## Required acceptance and approval boundary

The gate must retain **two consecutive real passes in Chromium and two in
Firefox**, each with `403 → 204 → 200 → 204 → 403`, against the same reviewed
runtime and original authorization model. A partial boot, native Go tests,
Dev Container tests or a single browser pass cannot satisfy this requirement.

The workflow named **Browser preview** (`pages.yml`) runs this gate.
Run `43095508749`, named **Browser lab verification**, is a different,
native Dev Container workflow and cannot prove browser VM execution.

The user has explicitly approved publishing this fix and additional checks.
The expired source artifact required repeating its original conversion job at
the same pinned source commit; a new build must be recorded with its own hashes,
not treated as byte-identical to the expired artifact. Merging and publishing
Pages still require confirmed successful browser acceptance and separate user
permission. This document does not authorize automatic merge or publication.
