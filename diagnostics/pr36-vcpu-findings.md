# PR #36: one guest vCPU did not pass browser acceptance

Date: November 20, 2026.

**The one-vCPU configuration is not a confirmed correction. Both actual
one-vCPU browser VMs completed zero of the two required authorization vectors
and did not reach an observed OpenFGA startup. The two-vCPU controls also failed,
at later startup RPCs. No production acceptance or publication gate passed.**

This is a functional result for these executions. The GitHub-hosted machines
were not identical, so it is not an isolated CPU performance measurement or
proof of the precise cause of the historical Check failure.

## Completed runs

- [Four-case matrix: 44663922895](https://github.com/qqrm/openfga-axum-lab/actions/runs/44663922895),
  workflow `d4794e07c8e9cefef6271f498c6c309ce52b661c`.
- [Firefox/one-vCPU setup replacement: 44664594362](https://github.com/qqrm/openfga-axum-lab/actions/runs/44664594362),
  workflow `b2d46dde31a69f0ab34229a2d76ddcacf1a066b0`.

Each actual VM was required to complete two consecutive, distinct, indexed
executions of `403 -> 204 -> 200 -> 204 -> 403`. None completed its first vector.
The guest's `checks=0` field counts completed acceptance executions; it must not
be confused with the number of real OpenFGA Check RPCs.

| Browser / guest vCPUs | Observed result | Acceptance vectors | Job |
| --- | --- | --- | --- |
| Chromium / 1 | PostgreSQL emitted `server started`. Subsequent database setup was not observed to reach the OpenFGA start marker. First host wait expired at 1,200,000 ms. | 0/2 | [136048513540](https://github.com/qqrm/openfga-axum-lab/actions/runs/44663922895/job/136048513540) |
| Chromium / 2 | Initial real bootstrap deny Check succeeded in 2720 ms. A subsequent tuple `Write` returned `Code(4004)`, `context_state=deadline`, after 3087 ms; HTTP returned 500. | 0/2 | [136048513861](https://github.com/qqrm/openfga-axum-lab/actions/runs/44663922895/job/136048513861) |
| Firefox / 1 | Diagnostic `initdb.log` ended at `performing post-bootstrap initialization ...`. No OpenFGA start or RPC was observed. Guest emitted `LAB\|boot-failed`. | 0/2 | [136050712389](https://github.com/qqrm/openfga-axum-lab/actions/runs/44664594362/job/136050712389) |
| Firefox / 2 | `WriteAuthorizationModel` returned `Code(4004)`, `context_state=deadline`, after 3325 ms. Model POST returned HTTP 500 instead of 201. No Check RPC was reached. | 0/2 | [136048513992](https://github.com/qqrm/openfga-axum-lab/actions/runs/44663922895/job/136048513992) |

The RPC durations in the table are guest service measurements. They are not
host receive intervals. The configured OpenFGA request deadline remained three
seconds; cooperative cancellation can finish after that deadline.

## What was held constant and verified

The [experiment declaration](pr36-vcpu-experiment.md) was committed before the
run. The control used runtime `444b069daf16e9606a0bbf1ea0aea7f4d61b97d8` with the
already-built idle-worker GC emulator from
`6f35f24b7ca345cf47c873becee6a41ef94e65a9`. This is the newer baseline that had
reached real Check failures, rather than the older silent-stall configuration.

All cases reused the same verified JS/WASM emulator pair, Linux kernel,
original root filesystem, PostgreSQL, OpenFGA and Rust binaries, pinned model,
512 MiB guest memory, MTTCG accelerator, RNG and service/acceptance deadlines.
All seventeen native producer inputs were verified unchanged. No native or
emulator rebuild was needed.

The one-vCPU overlay used the exact two-file delta from
`e0e577d25103cd4d8013fcccafa8db4e3a8bd601`: launcher topology and guest CPU guards,
plus the necessary served-file integrity hashes. The two-vCPU control preserved
every served byte after the common emulator overlay.

For **all four actual VMs**, the independently reconstructed source logs and
result JSON confirmed:

- The requested launcher topology, Linux online CPU list and service CPU
  availability agreed: `0` for one CPU and `0-1` for two.
- The host reported four logical CPUs; a one-vCPU guest was not a one-CPU host.
- Complete, contiguous source capture reconstructed the exact console bytes.
- Console, stream and result SHA-256 values matched the checker summary.
- `pageErrors=[]`, `ready=false`, `accepted=false`, and no acceptance vectors.

Workflow syntax/actionlint and the overlay boundary checks passed before
execution. Independent review also checked the source/producer identities and
the final raw evidence. These checks do not turn the failed VM results into a
successful authorization gate.

## Chromium/one-vCPU progress and its limits

The complete console contains `server started` at original line 377, JSONL
batch line 231, received at **02:58:20.261 UTC**. The entrypoint runs `initdb`
before this `pg_ctl -w start`, then two uninstrumented `createdb` commands before
the OpenFGA start marker. It is therefore incorrect to say that this case never
completed `initdb` or never started PostgreSQL. The logs do not isolate which
later database operation or scheduling condition prevented progress.

The last source/JIT snapshot arrived at **03:03:05.162 UTC**, another 4 minutes
44.901 seconds after `server started`. No later nonempty source batch arrived
for **694,964 ms**, until the checker ended at **03:14:40.126 UTC**.
These are host receipt timestamps, not evidence that the guest clock stopped.

The last core-0 snapshot recorded 4,742,144 cumulative registrations,
4,740,096 detachments and 4,733,952 GC callbacks. At that observation the
differences were 2048 retained, 6144 detached awaiting callbacks and 8192 total
unfinalized instances, against a configured limit of 15,000. The observation
was already over eleven minutes old at checker completion. It does not show
that the shared limit was reached, establish live memory consumption, or prove
a deadlock. Millions of cumulative registrations are not millions of live or
unique modules.

## Firefox/one-vCPU failure boundary

The failure snapshot shows incomplete `initdb` post-bootstrap initialization,
followed by a generic guest failure marker. The observed interval from
`LAB|boot` to the diagnostic snapshot was 906.109 host seconds, followed by
8.983 seconds until `LAB|boot-failed`.

This timing is consistent with the existing 900-second guest readiness guard
and the bounded diagnostic subprocess. However, the same guest branch is also
used if the child service process exits, and it records no separate reason tag.
The exact branch trigger is not established. Complete transport capture also
does not mean the bounded diagnostic subprocess completed every command: it
did not reach the guest uptime/process snapshot after its memory output.

## Host differences limit causal conclusions

| Case | Host processor | Hypervisor | Host logical CPUs |
| --- | --- | --- | --- |
| Chromium / 1 | Intel Xeon 6975P-C | KVM | 4 |
| Chromium / 2 | AMD EPYC 7763 | Microsoft | 4 |
| Firefox / 1 | AMD EPYC 7763 | Microsoft | 4 |
| Firefox / 2 | AMD EPYC 9V74 | Microsoft | 4 |

Both Chromium cases used version `157.0.8103.50` and the exact same executable
SHA-256, `cce8581219e59c17bcc60bff0cdfdcb2405a6d63acb1f3c32670a40814ba558c`.
Both Firefox cases used the locked `159.0` browser. Matching browser bytes does
not remove differences between host processors, hypervisors or load.

Reducing guest CPUs also changes default service parallelism and the aggregate
per-vCPU retained JIT capacity. This run consequently supports the narrow
finding that one vCPU did not provide the required mitigation here. It does not
rank one versus two CPUs by speed, isolate an MTTCG race, prove MTTCG innocent,
or establish a universal failure rate.

## Excluded setup failure

The initial Firefox/one-vCPU job `136048514037` hit the three-minute apt install
limit before launching any browser VM. It is excluded from runtime results.
The separate replacement ran only that missing case, retaining all runtime
inputs, service deadlines and acceptance waits. It allowed six minutes for
package installation, eight minutes from the actual job start to launch, and
a 35-minute outer job. The original runtime waits remained 20 + 3 minutes.
The other three VM cases were not repeated.

## Retained evidence and reproduction

The [artifact manifest](pr36-vcpu-evidence/artifacts.json) records the exact run,
job, artifact ID, archive size and SHA-256 for every actual case. Raw console,
stream, result, screenshot, topology proof, source/producer provenance and host
identity remain in those GitHub Actions artifacts.

| Case | Artifact ID | Verified archive SHA-256 |
| --- | --- | --- |
| Chromium / 1 | `14598390066` | `f87a249f3833f4e6e69337c6f3c39aef463b74d86b3fd012fb882f8845994247` |
| Chromium / 2 | `14596469669` | `03e83f9b2d233eff6f9ca9ca820a0f614edd0a24182b7c46d5e2781cba844266` |
| Firefox / 1 | `14597798979` | `a3aef30b50d34b29db47d6f729b6b0535aa074b332a0b8d7c3cdcc5a6cbb658e` |
| Firefox / 2 | `14597377997` | `4fca6dab1cdd9e883f52c272a7382b75f544eb81e58fbbbd8ef247c16cb220bf` |

Four `*-analysis.json` files in [pr36-vcpu-evidence](pr36-vcpu-evidence) contain
the verified original service events and exact source-line references. Four
`*-telemetry.json` files retain per-core observations and source gaps. Service
replays are excluded from original RPC results; telemetry observations are
never summed across stale per-core snapshots as an atomic shared counter.

After downloading a raw case artifact, the included readers can be run with:

```bash
python3 diagnostics/pr36-vcpu-evidence/analyze-browser-evidence.py CASE.zip \
  --expected-sha256 SHA256 --out analysis.json
python3 diagnostics/pr36-vcpu-evidence/analyze-vcpu-telemetry.py CASE.zip \
  --engine chromium --output telemetry.json
```

Use `--engine firefox` for a Firefox archive. The checked-in reports shorten
only the local archive filename and add GitHub provenance; source records and
verification results are unchanged.

Artifact `14597878044`, `pr36-vcpu-reproduction-inputs`, retains the original
verified input ZIPs for 30 days. Their earlier source artifacts expire on
November 20. Its outer archive digest from GitHub metadata is
`9d21628183e1d211558959446bbb71d76ef3fcbe21d221cc8f9c4fc51edbaa20`.
This preserves the original bytes and producer identities, rather than claiming
a fresh build. The diagnostic workflow remains an immutable experiment, not a
permanent artifact recovery service.

## Next useful investigation

A stronger causal comparison would run one and two guest CPUs sequentially on
the same host, retaining separate VM captures. Before another long acceptance
run, the startup diagnostic should record its actual exit reason, guest time
and child-process state before lengthy log/memory dumps. The Chromium boundary
needs observations around the two `createdb` calls; Firefox needs the state of
the post-bootstrap `initdb` child. These observations would distinguish an
unfinished operation from scheduling or emulator progress problems without
changing authorization semantics or increasing service deadlines.

The published PR branch and Pages were not changed by this investigation.
The one-vCPU topology remains a diagnostic candidate, not a validated fix.
