# PR #36: isolate the real OpenFGA engine inside the browser

Declared November 20, 2026, before executing this component experiment.

## Question

Can the existing real OpenFGA binary load the unchanged Lakekeeper model and
complete two distinct ownership grant/revoke cycles inside the existing browser
Linux VM when PostgreSQL and Axum are not running?

The application is Rust, but its runtime Authorizer calls a separate OpenFGA
engine. The engine already runs inside the same browser VM on loopback; it is
not an external computation service. The limited legacy SQL ACL evaluator and
the existing WASM lifecycle/visualization code do not replace that FGA engine.
This architecture is unchanged across main `95ae17f8`, PR head `55fb2797` and
the tested runtime `444b069d`.

## Frozen inputs and finite scope

- Runtime source: `444b069daf16e9606a0bbf1ea0aea7f4d61b97d8`.
- Idle-worker GC emulator source: `6f35f24b7ca345cf47c873becee6a41ef94e65a9`.
- Same original root filesystem, Linux kernel, emulator JS/WASM, Rust bootstrap
  and OpenFGA 1.10.3 `.10` executables as the completed vCPU experiment.
- Same pinned model SHA-256:
  `a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d`.
- One Chromium VM, two guest CPUs, 512 MiB, MTTCG, the existing RNG configuration.
- No native/emulator rebuild. All seventeen native producer inputs remain exact.
- Only served `guest.sh` and its necessary startup/derived manifest hashes change.
- OpenFGA uses its real built-in memory datastore. PostgreSQL, Axum, SQL/FGA
  migrations and the full application acceptance CLI are not started.

The original input ZIPs can be recovered, if expired/removed, only from pinned
retained artifact `14597878044` in run `44663922895`. Original run/job identity,
outer mirror and inner ZIP hashes, and producer provenance remain checked.
Recovery does not relabel a failed producer workflow as successful.

## Operations

First run the existing verified bootstrap executable: create a real store,
upload the exact model, read it back, verify an empty tuple store, and require
the original real higher-consistency negative ownership Check.

Then use two distinct fixture pairs:

- `user:browser-memory-probe-1`, `lakekeeper_table:browser-memory-probe-1`.
- `user:browser-memory-probe-2`, `lakekeeper_table:browser-memory-probe-2`.

For each pair, call real OpenFGA APIs in this order:

1. Check `ownership`: `allowed:false`.
2. Write the direct ownership tuple.
3. Check the same relation: `allowed:true`.
4. Delete that tuple through Write.
5. Check again: `allowed:false`.

All ten operations must return HTTP 200; successful Write returns `{}`. These
are OpenFGA API operations, not the application's HTTP `403/204/200/204/403`
acceptance vector. Every Check uses the same actual model ID and
`HIGHER_CONSISTENCY`.

The pinned rootfs has no assumed HTTP/JSON client installation. A small Bash
HTTP/1.0 client uses `/dev/tcp` and complete close-delimited reads, bounded to
16 KiB. It preserves raw HTTP response bytes and the prepared request body.
The host independently checks status, framing, strict JSON, model/store/fixture
identity, operation order and all authorization decisions. This client is a
diagnostic change; no performance equivalence to Rust's reqwest is claimed.

## Budgets and evidence

OpenFGA's existing 60-second warmup and self-dial, 180-second listener budget,
three-second request deadline, bootstrap 30-second model POST/GET budget, and
five-second ordinary client requests remain. Setup/host time is separately
bounded: one 35-minute job, launch within eight minutes of the actual Actions
job start, one 20-minute host component wait, 22-minute checker step.

Guest records use the distinct `FGA_MEMORY` protocol. They never emit full
application readiness or acceptance success. A transport completion marker
alone cannot pass the component: the host must validate every real response.
Before bootstrap and after the cycles, process snapshots verify the staged
OpenFGA process and absence of PostgreSQL/Axum processes. These are point-in-time
observations, not a claim to have traced every process throughout execution.

The unchanged bounded source collector retains sequenced console chunks,
receipt timestamps, a byte-reconstructible JSONL stream, exact hashes, browser
errors, actual topology, and producer/consumer identities. Missing/invalid
capture or records cannot be interpreted as successful engine execution.
Failure reason, phase and guest time are emitted before optional diagnostics.

## Interpretation fixed before execution

- Success demonstrates these real direct-ownership operations in one Chromium
  VM with memory storage. It does not validate PostgreSQL, computed Lakekeeper
  relations, Axum, Firefox, sustained reliability or the production gate.
- Failure at a real operation demonstrates that PostgreSQL/Axum are not required
  for that observed failure. It does not identify a unique emulator defect.
- A setup, instrumentation or capture failure is inconclusive about the engine.
- Removing PostgreSQL/Axum also changes load and memory pressure. A successful
  isolated run does not prove PostgreSQL itself is defective.
- Hosts are not matched to prior runs. No speed ranking or causal performance
  claim will be made.

Production branch, acceptance conditions and publication are outside this
component experiment. Results will be recorded separately after execution.
