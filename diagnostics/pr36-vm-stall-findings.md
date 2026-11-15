# PR #36: browser VM stall localization

Investigation date: 2026-11-15 UTC. This is a diagnostic result, not a browser acceptance result or a production fix.

## Finding

The instrumented Chrome run localizes recurring source-output pauses to pressure in the QEMU Wasm JIT instance lifecycle. In all three pairs of vCPU snapshots, `registered_total - gc_total` reconstructs the configured 15,000-instance ceiling. Of those instances, 11,264–12,288 had already been detached from the active cache and were still awaiting finalization callbacks. The browser main page, both observed vCPU workers, and the QEMU monitor remained responsive. Subsequent GC accounting and new compilation bursts accompanied further guest startup progress.

This is strong evidence of budget/GC backpressure, not proof of a permanent deadlock or of one cause for every earlier browser failure. The pairs of worker snapshots are nearly simultaneous, not an atomic read of the global C counter. TCI dispatch counts and time spent in TCI were not measured.

## Scope and provenance

| Item | Value |
|---|---|
| Diagnostic workflow | [run 43667908564](https://github.com/qqrm/openfga-axum-lab/actions/runs/43667908564), [job 132671307776](https://github.com/qqrm/openfga-axum-lab/actions/runs/43667908564/job/132671307776) |
| Harness commit | `695ec1bd4dcb5680c74d23736e1e545dd071b8d4` |
| Runtime source | `444b069daf16e9606a0bbf1ea0aea7f4d61b97d8` |
| Base rootfs/application source | `99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58` |
| Browser | Installed Chrome `157.0.8103.50`, sandbox enabled |
| VM | 2 vCPU, 512 MiB; original WASM, kernel, rootfs, OpenFGA and bootstrap binaries |
| Changes to the served runtime | Two recorded JS-only observation hooks: allowlisted serial input and read-only worker lifecycle snapshots |
| Artifact | [14143163177](https://github.com/qqrm/openfga-axum-lab/actions/runs/43667908564/artifacts/14143163177), 3,784,155 bytes |
| Artifact SHA-256 | `22f4069461ce7196804e3586f821facc8e7b32f23ead8383de2261c2b13ab2c6` |

The earlier failed run used Chromium `158.0.8127.18`. The new system-Chrome run is therefore a control on the same VM inputs, not an exact browser-version reproduction. Bundled Chromium could not launch with its sandbox on the diagnostic runner; the successful run used the runner's installed Chrome without disabling the sandbox.

The capture is complete: 42,944 source chunks, 405 batches, 120,254 bytes. Reconstructing the console from JSONL gives byte-identical output with a continuous sequence. There were 44 successful main-page heartbeat observations, no page errors, and no main observation errors. Three other workers did not answer the first inspector evaluation within its two-second bound; their role and state were not established, so those timeouts are not classified as deadlocks.

## Three pauses at the live-instance ceiling

`R` = registered instances; `D` = instances detached from the active cache; `G` = delivered finalization callbacks. `R-G` counts instances still charged by this accounting; `D-G` counts detached instances still awaiting a finalization callback.

| Round / host UTC | Core | R | D | G | R-G | D-G |
|---|---:|---:|---:|---:|---:|---:|
| 1 / 02:31:06.932 | 0 | 14,331 | 12,288 | 12,288 | 2,043 | 0 |
| 1 | 1 | 28,317 | 26,624 | 15,360 | 12,957 | 11,264 |
| **1 total** | | | | | **15,000** | **11,264** |
| 2 / 02:32:41.984 | 0 | 26,298 | 24,576 | 18,432 | 7,866 | 6,144 |
| 2 | 1 | 39,902 | 37,888 | 32,768 | 7,134 | 5,120 |
| **2 total** | | | | | **15,000** | **11,264** |
| 3 / 02:35:41.815 | 0 | 80,962 | 79,872 | 72,704 | 8,258 | 7,168 |
| 3 | 1 | 96,854 | 95,232 | 90,112 | 6,742 | 5,120 |
| **3 total** | | | | | **15,000** | **12,288** |

The two vCPU snapshots in each round were separated by 9, 165, and 7 ms respectively. All six had `attempted_total == registered_total`, `gcCallbacksNotYetAccounted == 0`, and `removePending == 0`. The counts retained in the active cache were only 3,736, 3,736, and 2,712 respectively; the remainder was detached but not finalized. These are object/accounting counts, not a memory-byte measurement.

Before each round, no source chunk had arrived for 60.338, 60.032, and 60.098 host seconds. Before the first worker/HMP intervention, renderer PID 3336 accumulated 103.83 CPU seconds during 50.069 wall seconds while the source sequence remained 22,288. This establishes CPU activity during the quiet period, but does not distinguish useful guest execution, interpretation, or spin.

Every HMP round returned `VM status: running`, CPU registers, IRQ counters and PIC state. Guest RIP/CR3 values and the active/HLT roles changed between rounds, and disk IRQ counts increased. A constant PIC IRQ0 count is not evidence that the guest clock stopped; these statistics do not provide a complete local APIC timer trace.

GC release and compilation resumption are visible in the subsequent source stream:

- After round 1, core 1's `gc_total` rose from 15,360 to 26,624, accounting for the 11,264 instances that had been detached at the snapshot. The first new sparse JIT log arrived at 02:31:41.215.
- After round 2, core 0's `gc_total` rose from 18,432 to 24,576, accounting for 6,144 previously detached instances. The first new sparse JIT log arrived at 02:33:25.958.
- These resumed logs arrived 34.283 and 43.974 seconds after the corresponding intervention started. The observations do not establish that inspector/HMP caused GC or recovered the VM.

## Mechanism in the tested source

Relevant sources are the [pinned runtime patch](https://github.com/qqrm/openfga-axum-lab/blob/444b069daf16e9606a0bbf1ea0aea7f4d61b97d8/browser-single/qemu/instance-budget.patch) and [pinned upstream `tcg/wasm32.c`](https://github.com/ktock/qemu-wasm/blob/8604ed49a3cde392890b014a8d5a959c8a2fe72a/tcg/wasm32.c).

1. `try_reserve_instance()` checks finalizer accounting and atomically reserves from the shared live-instance limit before creating a new instance.
2. Detaching an instance from the active cache does not release its global credit. The credit is released after its `FinalizationRegistry` callback is delivered and `check_instance_garbage_collected()` processes it.
3. At the limit, a cache miss that cannot reserve a new instance executes `tcg_qemu_tb_exec_tci()`. Existing compiled blocks remain usable; reaching the limit does not switch every instruction to TCI.
4. `trysleep()` checks every 50,000 outer dispatch iterations and calls `emscripten_sleep(0)` when the budget is full. Yielding permits event-loop progress but does not guarantee prompt finalization.
5. Ordinary JIT diagnostics are emitted only every 1,024 successful registrations or on an exception. Interpretation can therefore continue with no JIT output.

This source path and the observations support investigating instance retention, finalizer-dependent credit reclamation, and fallback execution. They do not establish a browser GC bug, a C accounting bug, or the fraction of runtime spent interpreting.

## Guest startup reached beyond OpenFGA warmup

All times in this table are host receipt times, not guest timestamps.

| UTC | Observed event |
|---|---|
| 02:30:06.594 | PostgreSQL initialization begins |
| 02:32:50.524 | PostgreSQL reports `server started` |
| 02:33:19.813 | OpenFGA migration/start begins |
| 02:33:53.527 | OpenFGA `warmup_start` |
| 02:34:04.949 | `warmup_end duration_ms=11427 failed=false` |
| 02:34:09–21 | Real health, store/model operations, Read and the bootstrap deny Check finish successfully |
| 02:34:21.239 | Empty store and pinned model provisioned; SQL authorization migration still required |
| 02:34:22.299 | Rust/Axum process startup begins |
| 02:34:41.716 | Last guest log: a SQL migration slow-statement warning |
| 02:36:16.876 | Diagnostic stops after three monitor snapshots plus a final observation interval |

The last quiet period is around Rust application startup after SQL migration output, not inside the earlier OpenFGA warmup. The [application startup source](https://github.com/qqrm/openfga-axum-lab/blob/99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58/src/server.rs) proceeds through SQL migration, demo seeding, graph migration and additional startup work before `Listening on`. Demo seeding hashes five passwords with Argon2, and startup later hashes a dummy password. There are no per-phase markers proving which of these later operations was executing at the final snapshot; the user-mode RIP alone cannot identify that function.

## Earlier failures and Firefox boundary

The [original browser run 43503424618](https://github.com/qqrm/openfga-axum-lab/actions/runs/43503424618) failed the 1,200,000 ms host wait in both engines. Chromium's last service marker was PostgreSQL initialization; Firefox's was OpenFGA `warmup_start`. The final source-chunk-to-checker-finish gaps were 18m44.838s and 11m50.508s respectively. Sparse per-core counters in those old captures cannot be summed as an exact simultaneous global budget reading.

The [new Firefox job](https://github.com/qqrm/openfga-axum-lab/actions/runs/43667218123/job/132669166941) used Firefox `159.0` and the same pinned native/runtime inputs. PostgreSQL initialization to `server started` took 6m24.573s; the VM subsequently reached OpenFGA migration/start. JIT/GC logs continued until the eleven-minute host cutoff, with no page errors. The old prolonged silence was not reproduced in this window.

That Firefox run had no worker/HMP intervention: continuous JIT logs prevented the original 60-second total-silence trigger from firing. Consequently the Chrome budget localization is not a measured proof of the Firefox failure's cause. A locally prepared phase-aware observer separates service progress from JIT log activity, but it was not run against another VM and adds no new runtime evidence.

The existing guest startup watchdog uses guest time (900 seconds). It can be delayed by slow or stalled guest execution, and its process snapshot covers PostgreSQL processes only. Host-driven diagnostics are required to localize an emulator-level pause independently of guest progress.

## Status and next discriminator

Both diagnostic runs ended with `ready=false`, `accepted=false`, `checks=0`, and `acceptanceVerified=false`. The real acceptance sequence `403 → 204 → 200 → 204 → 403` has not passed. A green diagnostic job means the observation completed, not that the lab passed acceptance.

PR #36 remains open at `55fb279741aa47ae813e5b7d5aff73dbd2692cc1`; this investigation did not update the PR head, merge it, or publish Pages. Guest/application authorization behavior and deadlines were not relaxed.

The narrow next discriminator is direct shared C budget, TCI-dispatch and yield-entry/exit counters on the same two-vCPU runtime, optionally followed by an explicitly marked worker-GC intervention to test whether released credits restore compilation and service progress. A one-vCPU control is less specific because it also changes MTTCG parallel-code generation and scheduling. Raising deadlines or simply raising the instance cap would not establish the cause or validate a fix.
