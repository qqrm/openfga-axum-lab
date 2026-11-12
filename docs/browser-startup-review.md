# Browser OpenFGA startup review

Status of this review on 12 November 2026: **approved changes under validation;
browser acceptance has not passed**. The latest retained test below is commit
`077452deabadde3c06bce44a49e8e0b4e5c5febf`, run `43318362131`.
The Browser preview run for the current PR head is the authority for later
acceptance results. The initial proposal was prepared against PR #36 head
`c527f3ca016b317350a29c0dca38c07e7699230b`. The user approved publishing changes
and additional checks. The PR has not been merged and Pages has not been
published.

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
- Before opening listeners, exercise real codecs and validation APIs for the
  exact pinned model in four fresh cycles under one 60-second startup budget,
  then discard the results. Record per-cycle stage timings.
  Do not cache validation decisions, perform backend writes, or skip any
  subsequent real request. Omit unary raw-payload logging work while retaining
  compact transport metadata and the existing security middleware.
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

The next [emulator candidate](../browser-single/qemu/README.md) changes only
`MAX_INSTANCE_ALIVE` from 15000 to 4096. It uses the existing eviction, GC and
TCI fallback; the compilation threshold remains 1500. `tb-size` is a different
buffer and its flush does not release those compiled-module references.
4096 is a measured-motivation candidate, not a proven safe byte budget or a
performance result. Its first actual build and browser gate remain necessary.

The OpenFGA candidate repeats fresh codec/validation work four times under the
same total 60-second startup budget. No TypeSystem or validation result is cached.
Every real request remains unchanged. Increasing administrative model RPC budgets
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
build or browser acceptance; those remain the next required CI evidence.

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
