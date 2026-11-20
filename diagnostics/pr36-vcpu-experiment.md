# PR #36: paired guest-vCPU experiment

## Question and scope

Does changing the real browser Linux VM from two guest vCPUs to one allow the
unchanged authorization gate to pass? The current baseline is the idle-worker
GC candidate tested in run
[43969702163](https://github.com/qqrm/openfga-axum-lab/actions/runs/43969702163).
Both browsers in that run used two vCPUs and failed real OpenFGA Check requests.
The older 20-minute silent-stall result is not the current control.

This tests a practical guest-topology change. It does not change the GitHub
runner's CPU allocation, pin host affinity, or set Go/Tokio thread counts.
One guest CPU can also change the services' default parallelism and aggregate
per-vCPU JIT retention; success would not isolate a particular MTTCG race.

## Immutable inputs

| Input | Identity |
| --- | --- |
| Runtime source | `444b069daf16e9606a0bbf1ea0aea7f4d61b97d8` |
| Emulator source | `6f35f24b7ca345cf47c873becee6a41ef94e65a9` |
| Emulator producer | Run `43969702163`, attempt 1, workflow `a12b8799fed16e6efc93ab55498e8f165fb1a139` |
| Successful emulator build job | `133658307719` |
| Retained emulator artifact | `14276419843`, 16,286,946 bytes |
| Emulator archive SHA-256 | `6fcd65f16a8a05207b7a15460ae2097ecafa00ee031fcc4c60bda3edf5e1ea30` |
| One-vCPU source delta | `e0e577d25103cd4d8013fcccafa8db4e3a8bd601`, parent exactly the runtime source |
| Native OpenFGA/bootstrap proof | Run `43500949883`, attempt 1 |

The emulator is restored from the successful build job of an overall failed
browser run. Its original producer provenance remains unchanged. Current
consumer run identity is recorded separately. No emulator or application
rebuild is needed for this experiment.

The existing runtime materializer and all seventeen native-input checks remain.
After the verified emulator overlay, the one-vCPU case copies only the pinned
launcher and guest CPU guards; the guest hash in the startup manifest and its
derived manifest reference are updated. All other served bytes are compared.
The two-vCPU control retains every served byte after the emulator overlay.

## Finite comparison

One workflow runs four jobs: Chromium with one and two vCPUs, and Firefox with
one and two vCPUs. Each starts at most one Linux VM. Each VM must complete two
distinct, consecutive, indexed real authorization vectors:

`403 -> 204 -> 200 -> 204 -> 403`

The kernel, original root filesystem, application and emulator binaries,
512 MiB guest memory, MTTCG accelerator, RNG, pinned model and every service and
acceptance deadline are identical. The original 20-minute first host wait and
3-minute second wait remain. A failed guest is not retried. The production
checker retains its assertions; only its engine selection and the previously
reviewed system-Chrome sandbox/channel selection are transformed.

Observed guest topology is reported even when acceptance fails: launcher
evidence, Linux online CPU list, and service CPU affinity must agree. Browser
versions and host CPU/memory records are retained for comparison. No diagnostic
HMP commands or worker evaluations run inside the acceptance workload.

## Interpretation declared before execution

- One-vCPU failure with valid topology and complete capture means topology
  reduction alone did not solve the observed case.
- One-vCPU success with two-vCPU failure in the same browser supports that
  configuration as a mitigation in this run. It does not establish a unique
  mechanism or a general performance improvement.
- Success in both configurations does not establish that two vCPUs caused the
  historical failure.
- Failure before VM startup, missing topology proof, or incomplete capture is
  inconclusive for the runtime hypothesis.
- Chromium and Firefox are evaluated separately; one cannot substitute for
  the other. A single paired run is not a reliability or performance study.

The published PR branch and Pages remain unchanged by this diagnostic workflow.
Native tests, Linux boot, successful startup operations, or successful evidence
collection cannot replace the two real vectors in both browser engines.

## Evidence retention

Each job retains its complete console, sequenced stream, result JSON, checker
summary, topology proof, materialization and producer/consumer provenance.
The Chromium one-vCPU job additionally retains the original verified input ZIPs
for 30 days because their source artifacts expire on November 20, 2026.
Retaining those bytes does not relabel the original producers or make a failed
browser gate successful.

Results are recorded in a separate findings file after the finite run completes.
