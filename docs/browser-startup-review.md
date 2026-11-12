# Browser OpenFGA startup review

Status at patch preparation: **approved fix under validation; browser acceptance
had not passed**. The Browser preview run for the current PR head is the
authority for subsequent acceptance results.
Prepared against PR #36 head `c527f3ca016b317350a29c0dca38c07e7699230b`
on 12 November 2026. The user approved publishing the changes and additional
checks after reviewing the patch and its failed Chromium result. The PR has
not been merged and Pages has not been published.

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

- Build OpenFGA from the exact upstream commit with Go 1.24.6. Change only
  the gateway startup self-dial budget from three to 60 seconds, observing
  parent cancellation. Preserve `grpc.WithBlock()`, TLS options and fatal
  startup failure. Record source, patch, compiler and executable provenance.
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

PostgreSQL, Axum, `lab-tool acceptance` and the pinned Lakekeeper model stay
unchanged. No response is synthesized. Existing deadlines remain unchanged:
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
later per-operation timeout. The exact operation still needs retained phase
diagnostics. Do not increase authorization or operation deadlines to conceal
this remaining limit.

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
and Axum startup. That final diagnostic wrapper has passed static checks but
has not completed another VM run.

## Narrow performance candidate

The original QEMU command line enables `slub_debug=F` for all kernel slab
caches. Linux documents its performance cost; the implementation moves
debug caches out of the allocation fast path and disables lockless freelists.
The pinned kernel configuration has `CONFIG_SLUB_DEBUG_ON` disabled.
Removing that explicit debugging token is a candidate that preserves the
application's deadlines, authorization behavior and PostgreSQL durability.
Its effect on this workload has not been measured.

The approved launcher selects two guest CPUs and removes only that allocator
debug token, with assertions against the reviewed original arguments. It keeps
512 MiB guest memory and all request deadlines. Syntax and applicability checks
passed after recovery; the combined behavior still requires real browser
acceptance. The earlier one-CPU failure is not evidence for this configuration.

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
