# PR #36: real OpenFGA completed two authorization cycles inside Chromium

Date: November 20, 2026.

**The existing OpenFGA executable works for the tested direct-ownership
operations inside the browser Linux VM. With its real memory datastore and
without PostgreSQL or Axum, it loaded the pinned model and completed two
distinct deny/grant/allow/revoke/deny cycles. All ten HTTP operations returned
200, and all six Check decisions were correct.**

This is a component result. The full application gate in PR #36 remains
unpassed. The earlier one-vCPU experiment did not provide a working correction.

## Execution and retained evidence

- Successful [run 44673550930](https://github.com/qqrm/openfga-axum-lab/actions/runs/44673550930),
  [job 136079905760](https://github.com/qqrm/openfga-axum-lab/actions/runs/44673550930/job/136079905760),
  attempt 1.
- Diagnostic workflow commit: `d1c20a5b205d5786bb56623eb28e62c2a842445c`.
- Artifact: `14601510410`, `pr36-openfga-memory-component`, 20,037,946 bytes.
- Verified ZIP SHA-256:
  `2c93481cb377043724547708255efed0df400f7f6946cb4d32bc8cd7b541c7de`.
- Checker interval: `04:19:28.104` to `04:21:35.962 UTC`, 127,858 host
  milliseconds. This includes browser/VM startup and evidence collection; it
  is not an individual RPC latency measurement.
- Guest: x86_64, two CPUs, Linux online and process affinity both `0-1`,
  512 MiB, the existing MTTCG and RNG configuration.
- Host: four logical CPUs, AMD EPYC 9V45, Microsoft hypervisor. This differs
  from the hosts of the previous vCPU cases, so elapsed times are not a
  controlled performance comparison.

The [experiment declaration](pr36-memory-component-experiment.md) fixed the
workload and interpretation before execution. [Retained text evidence](pr36-memory-evidence)
includes the original console, sequenced JSONL, checker result, overlay proof,
summary, host identity and independently derived analysis. These text records
remain in git after the Actions artifact expires. The full ZIP also includes
the screenshot and original input/producer provenance.

## What actually passed

The existing Rust bootstrap created a fresh store, uploaded and read back the
unchanged model, checked the empty tuple store and performed its original
negative ownership Check. The model SHA-256 was
`a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d`.

Actual binding:

- Store: `01M7VJXSGHAFC93RJ9HGD0KVDV`.
- Model: `01M7VJXV0BTTT847JBH9K2RBWS`.

The subsequent operations used distinct fixture pairs:

- Cycle 1: `user:browser-memory-probe-1` and
  `lakekeeper_table:browser-memory-probe-1`.
- Cycle 2: `user:browser-memory-probe-2` and
  `lakekeeper_table:browser-memory-probe-2`.

Every request used this actual store/model binding; Checks requested
`HIGHER_CONSISTENCY` on the direct `ownership` relation.

| Operation | Cycle 1 | Cycle 2 |
| --- | --- | --- |
| Check before grant | HTTP 200, `allowed:false` | HTTP 200, `allowed:false` |
| Write ownership tuple | HTTP 200, `{}` | HTTP 200, `{}` |
| Check after grant | HTTP 200, `allowed:true` | HTTP 200, `allowed:true` |
| Write tuple deletion | HTTP 200, `{}` | HTTP 200, `{}` |
| Check after revoke | HTTP 200, `allowed:false` | HTTP 200, `allowed:false` |

These are OpenFGA API responses. They are not the Axum application's
`403 -> 204 -> 200 -> 204 -> 403` acceptance vector. All ten transports exited
successfully within the existing five-second client budget; the existing
three-second OpenFGA request deadline was unchanged.

Before bootstrap and after both cycles, the process snapshots each contained
six processes, including PID 13 at `/tmp/browser-lab-runtime/openfga`, and no
PostgreSQL/Axum process. These are point-in-time observations, not continuous
process tracing. The committed component script does not start those services.

## Preserved runtime and scope of the change

- Runtime source: `444b069daf16e9606a0bbf1ea0aea7f4d61b97d8`.
- Idle-worker-GC emulator source: `6f35f24b7ca345cf47c873becee6a41ef94e65a9`.
- Same `.10` OpenFGA 1.10.3 executable used in the prior vCPU experiment:
  `d779834ae225c95502b957efc96a6153937bb2f5f2a1cef9f134847ae8c0d02b`.
- Same Rust bootstrap executable:
  `e1143b7d5f8cc564e09d12c8f685bd7cf33bd0beb1505f2b7cfc1f6f53e1d275`.
- All seventeen native producer inputs, rootfs, Linux kernel, emulator JS/WASM,
  production launcher and application binaries remained unchanged.
- Only the served guest orchestration and its necessary startup/derived
  integrity hashes changed. The guest selected OpenFGA's built-in memory
  datastore and omitted PostgreSQL, Axum and their migrations.
- Ordinary API operations used the declared bounded Bash HTTP/1.0 diagnostic
  client. This does not prove performance equivalence to the application's
  Rust HTTP/gRPC client.

The original producer identities and archive hashes were verified before the
run. The retained-mirror fallback was not used in this successful execution.
The prelaunch job-budget check passed at 40,916 ms after the actual job start.
The observed guest boot record arrived less than two minutes after job start.

## Independent evidence verification

The downloaded ZIP matched the Actions digest and size. Each of the three
files in the original checker summary matched its declared byte count and
SHA-256. Reconstructing the JSONL's 46,120 contiguous source chunks reproduced
the console byte-for-byte: 62,859 bytes across 327 delivered batches.

Replaying the component parser over the original chunks reproduced all 31
event/data/source-sequence records and the final capture counters exactly.
Its demultiplexer accounted for 54 complete QEMU telemetry chunks containing
16,741 bytes; all those bytes remain in the original source evidence.
There was no partial component record at the capture cutoff.

A separate Python review independently decoded the ten raw HTTP responses,
checked framing and body lengths, matched the store/model response headers,
and found ten distinct request IDs. It confirmed the six decisions and four
successful writes. The JS verifier independently checked the prepared request
bodies, operation order, identity, strict JSON and process snapshots.

The capture was complete, `pageErrors=[]`, and no guest runtime error was
present. The production page deliberately remained `ready=false`,
`accepted=false`, `checks=0`; `productionAcceptance=false` throughout.
The protocol regression suite passed 25 tests before the run.

## Excluded attempts and instrumentation correction

- Run `44672544748` stopped at ShellCheck before downloads or a VM start.
  Explicit empty/literal shell assignments fixed the four warnings.
- Run `44672651991` booted Linux but the new component parser failed before
  observed OpenFGA startup. It had concatenated UART and QEMU diagnostics;
  a complete telemetry chunk landed between a guest JSON object and its
  newline. This was an instrumentation failure, not an engine failure.

The correction demultiplexes only complete original QEMU telemetry chunks
before reconstructing guest lines. It preserves the raw collector and strict
whole-record JSON validation. The actual failing source chunks are committed
as a regression fixture. The successful run used this correction without
changing the guest workload, executable bytes or time budgets.

## What this means for the Rust application and PR #36

The Rust application calls OpenFGA to compute authorization decisions:
[`OpenFgaHttp::check` and `apply`](https://github.com/qqrm/openfga-axum-lab/blob/444b069daf16e9606a0bbf1ea0aea7f4d61b97d8/src/infrastructure.rs#L333-L404)
send real `/check` and `/write` requests. Its Rust implementation does not
eliminate this separate engine. The
[`browser-single` entrypoint](https://github.com/qqrm/openfga-axum-lab/blob/444b069daf16e9606a0bbf1ea0aea7f4d61b97d8/browser-single/entrypoint.sh#L19-L126)
already starts OpenFGA inside the same browser VM at `127.0.0.1:8080`.

The successful component run rules out the blanket claim that this OpenFGA
executable cannot perform real authorization in this browser emulator. It
does not establish sustained reliability, computed/recursive Lakekeeper
relations, Firefox compatibility, PostgreSQL-backed behavior or full Axum
acceptance. Removing services also changes contention and memory pressure,
and the host changed. This result therefore does not prove PostgreSQL is the
unique cause of the earlier failures.

The [one-vCPU result](pr36-vcpu-findings.md) remains unchanged: neither actual
one-vCPU case passed the full gate, and neither reached observed OpenFGA
startup. This successful component run used two guest CPUs; it is not evidence
for the one-vCPU mitigation.

The next targeted investigation is the full stack's PostgreSQL initialization
and OpenFGA datastore/deadline path, using the successful component as a
reference. Removing OpenFGA or writing a replacement Rust policy evaluator is
not supported as a necessary remedy by these results. No production branch,
PR acceptance condition or Pages deployment was changed.
