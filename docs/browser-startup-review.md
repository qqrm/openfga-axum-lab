# Browser OpenFGA startup review

## Latest structural-path browser result — 2026-11-13

[Run 43483871350](https://github.com/qqrm/openfga-axum-lab/actions/runs/43483871350),
workflow `5083ad7458337baf3107bfd5537b934b91183a18`, runtime
`b04bad4ac109a5fdab185a9025b1ae3982953629`, attempt 1, completed FAILURE.

| Engine | Established result |
| --- | --- |
| Chromium 158.0.8127.18 | All eight preparation steps passed. Linux and PostgreSQL started. OpenFGA's single 60-second startup warmup failed before HTTP listener, self-dial or authorization Check. Zero acceptance passes; ready=false, checks=0, pageErrors=[]; complete capture. |
| Firefox | Job 132087064759 failed step 6 with `Current job start metadata identity mismatch` before artifact reads, browser installation or VM startup. No browser result or artifact exists. The old compound predicate did not log the differing field, so the cause is unestablished. |

Chromium job 132087065246 ran the checker from 18:35:11.865 to 18:44:43.297 UTC.
The actual OpenFGA markers report one cycle, `completed=false`,
`current_stage=typesystem`, JSON decode 2738 ms, protobuf 2042 ms, generated
validation 555 ms, unfinished TypeSystem 54245 ms, and response codec 0 ms.
The enclosing warmup ended after 60108 ms with failure. The repeated markers
in the diagnostic tail are replay, not retries. TypeSystem time includes
original validation and structural preparation; no completed-pair counter exists.

The retained artifact is `14067097286`, `browser-openfga9-paths-chromium`,
3684393 bytes, SHA256
`c90c5a36f7e6e8b1ab03bdd1b12db489d18d3ba6090d0235490634c65ea5323b`.
The complete capture reports 29803 chunks, 347 batches and 60944 console bytes,
SHA256 `b82edd6376065d99a50709e9e2f949a736fb4ca604751d0de1c7a804dfde9172`.
This summary uses the completed job's full 22 selected service observations and
structured result; it does not claim independent reconstruction of this console.

### Current .10 source candidate and pending verification

The [runtime source 444b069d](https://github.com/qqrm/openfga-axum-lab/commit/444b069daf16e9606a0bbf1ea0aea7f4d61b97d8)
and [native source bb8fed25](https://github.com/qqrm/openfga-axum-lab/commit/bb8fed250bdee9931f7702ae2475d28e5e9eed8f)
replace repeated schema graph searches with at most two existing Gonum
breadth-first walks, then membership checks for the same 316 target pairs.
Both sources have identical OpenFGA helper, tests and producer inputs.
The pinned language graph's original `PathExists` uses Gonum's
`topo.PathExistsIn`, which starts a new BFS for each query. The candidate walks
that same directed graph from `user`, and lazily from `user:*` only if needed
and present; it does not simplify the authorization model.

Per-target source-before-target lookup errors, optional wildcard error
suppression, zero-pair behavior and the 512-pair bound are preserved.
Cancellation is checked inside each walk and before final private publication;
no partial visited set or map is returned. Existing plain-user keys, original
fallbacks and the one-shot transfer after a real resolver model read are
unchanged. The map contains schema reachability, never tuple or authorization
results.

The four existing structural-path groups remain. The added
`TestBrowserModelUserPathsTwoWalkParity` checks all 316 results against the
original predicate, actual graph expansion counts and same-node reachability.
`TestBrowserModelUserPathsLookupFallbacks` covers source/target error order,
absent, present and damaged wildcard nodes, and zero pairs.
`TestBrowserModelUserPathsCanceledDuringWalk` cancels from a real graph
expansion and requires the helper to discard the partial walk. All seven
groups are required in the regular and focused race gates. Build metadata
declares `schema_reachability_preparation=two_breadth_first_walks` and
`schema_reachability_max_walks=2`; it is emitted only after tests pass.

At this source snapshot, .10 native verification is still pending and there is
no .10 browser result. The preceding .9 native success cannot validate the
changed preparation. Warmup remains 60 seconds, the listener supervisor
180 seconds, self-dial 60 seconds, OpenFGA authorization three seconds,
Axum authorization two seconds and the real bootstrap/acceptance Check five
seconds. Fresh native parity/race and real-service proof, then two consecutive
original acceptance sequences in each browser, are still required.

The separate [Actions metadata repair](https://github.com/qqrm/openfga-axum-lab/commit/05d5abc1458e677c7ec634e02e2e3a27c30d5f0b)
uses an exact run attempt, bounded observations and the actual API job start
time. It remains inactive. The discarded 180/300-second budget fixture is also
inactive and is not part of this runtime. PR merge and Pages remain held.

## Previous VirtIO RNG browser result — 2026-11-13

[Run 43476279151](https://github.com/qqrm/openfga-axum-lab/actions/runs/43476279151),
workflow `0b7431bf861d2b96ef305620f10fc619832ab961`, runtime
`716b77356dd4aab2747d4e37c5da723f3b7a764f`, attempt 1, completed with
**zero of two required acceptance passes in each browser**. Every pinned input,
native/OpenFGA proof, lightweight transport proof, kernel configuration and
runtime segment check passed before the VMs started. The unchanged checker
launched one VM per engine and retained complete captures with no page errors.

| Engine | Current observed boundary |
| --- | --- |
| Chromium 158.0.8127.18 | CRNG is ready at guest 62.293280 s before bootstrap. OpenFGA warmup (17808 ms), self-dial (387 ms) and the real bootstrap deny Check (2095 ms) succeed. A real migration tuple Write succeeds (2606 ms RPC / 2877 ms HTTP). The first migration Check fails with Code 4004/deadline at 3651 ms, HTTP 500 after 4057 ms. Readiness and acceptance remain false. |
| Firefox 159.0 | CRNG is ready before guest bootstrap. PostgreSQL and OpenFGA start, and actual model POST/GET plus empty tuple Read succeed. The first bootstrap ownership Check fails at 3101 ms with Code 4004/deadline, HTTP 500 after 3393 ms. Axum does not start. |

The [Firefox reader 43479027853](https://github.com/qqrm/openfga-axum-lab/actions/runs/43479027853)
reconstructed every JSONL chunk and verified counts, cutoff and absence of a
capture error. Two independent decodings of its 73 base64 parts reproduced
all 272613 console bytes, SHA256
`9c187ae97395987082095b5a089443141b65c10a937ef4f85a3c07a6193e2700`.

The fresh Firefox console records `[109.388208] random: crng init done`,
received at 17:50:27.524 UTC. `LAB|boot` follows at 17:50:48.175 UTC.
PostgreSQL reports readiness in its own journal at 17:55:08.545 UTC.
OpenFGA warmup succeeds in 18460 ms and the internal gRPC self-dial in 1218 ms.

Before the first `Guest diagnostic:` header, there are seven completed RPCs:
Health (750 ms), CreateStore (489), WriteAuthorizationModel (2652), GetStore
(202), ReadAuthorizationModel (1728), Read (1366), and the failing Check (3101).
The 217-byte Check is the real empty-store bootstrap deny probe, with the
prepared TypeSystem claimed. There is no tuple Write or Axum startup in that
fresh prefix. The later startup-log tail repeats these records; it is not a
retry. PostgreSQL subsequently receives normal fast shutdown after the bootstrap
error. This run establishes that the earlier entropy wait no longer blocks
PostgreSQL, while genuine authorization still exceeds its original deadline.

The [Chromium reader 43479582636](https://github.com/qqrm/openfga-axum-lab/actions/runs/43479582636)
also passed full reconstruction and cutoff checks. Two independent decodings
of 29 base64 parts matched 107164 bytes, SHA256
`1f6b55b7151d8ee4480393fd740b361d819527daac54ede333ccbed0306297ad`.
The fresh prefix contains exactly two OpenFGA Check pairs: successful bootstrap
and failed first migration Check. The later diagnostic tail adds no requests.
Chromium already reached CRNG readiness in previous runs; this observation is
not evidence of an earlier Chromium entropy blockage or a controlled speedup.

The new package was independently reconstructed before materialization; every
non-kernel segment, including the original 546670592-byte rootfs, stayed exact.
The resulting data is 550773312 bytes, SHA256
`7863b9794e6cc69f7661056ecdbc42439b1143febba5294812ca203491df5a2f`;
the 7521-byte loader has SHA256
`17d523c59fb6f7efd8b42ea03877c21bb0e3fa76dbef3486568d37dec677a078`.

### Next bounded adaptation and production compatibility

The [structural-path .9 source](https://github.com/qqrm/openfga-axum-lab/commit/b04bad4ac109a5fdab185a9025b1ae3982953629)
completed [fresh native verification 43478795296](https://github.com/qqrm/openfga-axum-lab/actions/runs/43478795296)
at workflow `16266f3990af32a375039ce945322c26fa44b58c`. It can privately
precompute at most 512 original schema predicates (316 for this pinned model)
inside the existing 60-second startup phase. Only plain-user structural paths
are reused; real model existence, tuple reads, rewrites and decisions remain.
Cancellation or an error prevents partial publication. Other subject types,
usersets and unknown target keys retain the original method and errors.
Fresh Go build, the ordinary regressions and focused race checks passed,
including all four new structural-path tests. The complete real native producer
then passed eight Node and 25 Rust tests, a genuine empty-store deny Check and
two original native acceptance sequences. Its six-second hold of an actual
Check response produced exit 1 after 5089 ms with no configuration published.
Default model POST/GET likewise failed closed after 5068/5072 ms.

The tested OpenFGA binary is 63074488 bytes, SHA256
`7841a91b524fc1be097f2f816a8ed39f502e1372bbf96300dc9008b3336f98c3`.
Its 2910-byte build record has SHA256
`d430aad43f10805ee655b8180fd2c25cc0a65eaaff1da57b37d3f556ad19f983`;
the 37910-byte race log has SHA256
`57d2b1550eb49cf2cd0d2113277dae1dec0750eee888b4a68bc0c059d9991a82`.
The bootstrap remains the exact 7444640-byte `e1143b7d...` output tested earlier.
Native evidence artifact 14065112277 is 24001397 ZIP bytes, SHA256
`a34151ad1bac812410c57d42ee93b50c5f50ca96c70299b43431ab9539bcca61`;
bootstrap artifact 14065566791 is 3321947 bytes, SHA256
`ae6ff35cec4281e13343f9f2b9def30200a803fcdaba0380e816e55b5402ab19`.

Native warmup took 57 ms; TypeSystem validation plus path preparation accounted
for 49 ms. Those native numbers did not predict the following .9 browser result,
recorded above: Chromium exhausted the unchanged 60-second startup budget.

The separate [production compatibility source d5638bcd](https://github.com/qqrm/openfga-axum-lab/commit/d5638bcdc9ef51fded0b711bcd979ff306eaa938)
removes exactly the three temporary syscall diagnostic lines, restoring the
6271-byte guest fixture and all six strict native host-file comparisons.
The other change is a bounded Pages-workflow step that can retrieve the same
original runtime ZIP from retained evidence. It verifies the fixed outer ZIP,
extracts only the fixed inner member, verifies that ZIP against the existing
manifest, and supplies the existing `BROWSER_RUNTIME_ARCHIVE` input. An unavailable
archive leaves the ordinary resolver intact; a successful download with an
integrity mismatch fails. Authorization and deployment gates are unchanged.
These sources are not yet the PR head, and nothing has been merged or deployed.

## Previous coalesced browser result and identified entropy wait

[Run 43471167424](https://github.com/qqrm/openfga-axum-lab/actions/runs/43471167424)
tested source `037a3c22a5fab45a735a6bfea9792c68049a4b62` with the checked
log coalescing and failure-only syscall snapshot. Both engines completed with
**zero of two required acceptance passes**, complete captures and no page errors.

Chromium started PostgreSQL and OpenFGA. Its real initial deny Check succeeded
in 1827 ms and claimed the prepared TypeSystem. The original migration Write
succeeded in 2014 ms; the first migration Check succeeded in 2720 ms; the second
exceeded the unchanged three-second server deadline (3216 ms, Code 4004; HTTP 500
after 3696 ms). Startup warmup was 12788 ms; the internal gRPC dial was 366 ms.
The full pre-diagnostic console establishes this count independently of later
log-tail replays. Coalescing delivered 61340 original chunks in 482 bindings,
but this result does not establish an authorization speedup.

Firefox stopped in initdb at selecting default max_connections. Its final process
snapshot identified postgres PID 26 sleeping in x86_64 syscall 318,
`getrandom(buf,48,0)`. Initdb PID 18 was waiting for its child with `wait4`.
There was no CRNG-ready message in the full console. Guest uptime was 981.93 s
and available memory 404084 kB. This is a concrete observation of a blocking
randomness request at failure, not proof that the same call occupied every prior
second. The capture contains 25900 original chunks in 1247 bindings.

The [Chromium reader 43473392717](https://github.com/qqrm/openfga-axum-lab/actions/runs/43473392717)
and [Firefox reader 43474557036](https://github.com/qqrm/openfga-axum-lab/actions/runs/43474557036)
verified ZIP/member hashes and contiguous JSONL reconstruction. Independent
base64 decoding reproduced the complete console bytes:

| Engine | Bytes | SHA256 |
| --- | ---: | --- |
| Chromium | 118872 | `9a1116365f3fe1217ddb5d00fc7b8e121b5cc41680eb9d34c7da56fbe4ce89a5` |
| Firefox | 361860 | `23258eb31613a9ab7897ee5e4344e858636d1cc46ae33905297c7190647df73d` |

PostgreSQL 17.11 calls `InitProcessGlobals` and its strong-random seed path during
`postgres --check`, before shared-memory and semaphore initialization.
The OpenSSL path can block in Linux `getrandom(...,0)` until CRNG readiness.
See the pinned PostgreSQL
[postmaster initialization](https://github.com/postgres/postgres/blob/REL_17_11/src/backend/postmaster/postmaster.c),
[standalone initialization](https://github.com/postgres/postgres/blob/REL_17_11/src/backend/utils/init/miscinit.c)
and [strong-random implementation](https://github.com/postgres/postgres/blob/REL_17_11/src/port/pg_strong_random.c).

### Prepared VirtIO entropy configuration

[Kernel reader 43471802815](https://github.com/qqrm/openfga-axum-lab/actions/runs/43471802815)
verified the actual old 75338-byte config, SHA256
`fbee1ad0bc5fb73f2ca82ba98fa045b76ff19e67a148b40cb93b63916c2ad9b8`:
`HW_RANDOM=y` and `VIRTIO_PCI=y`, but `HW_RANDOM_VIRTIO=n`.

Source [716b7735](https://github.com/qqrm/openfga-axum-lab/commit/716b77356dd4aab2747d4e37c5da723f3b7a764f)
enables that driver and appends an explicit QEMU `rng-random` backend reading
Emscripten's Web-Crypto-backed `/dev/urandom`, attached to `virtio-rng-pci`.
The launcher requires Web Crypto and rejects deterministic or conflicting RNG
arguments. Linux still performs its ordinary entropy-credit and getrandom
readiness checks; no weak seed, cryptographic bypass or authorization setting
is introduced. The pinned
[VirtIO driver](https://github.com/torvalds/linux/blob/830b3c68c1fb1e9176028d02ef86f3cf76aa2476/drivers/char/hw_random/virtio-rng.c)
uses the normal hardware-RNG interface and quality accounting.

[Kernel-only build 43473084506](https://github.com/qqrm/openfga-axum-lab/actions/runs/43473084506)
passed source, syntax and ShellCheck validation. It checked that the generated
config is byte-for-byte the old checked config after replacing exactly one
`HW_RANDOM_VIRTIO` line. IPC, ACPI, HPET, SMP, APIC and timer settings remain
unchanged. Artifact 14061334425 is 3689588 ZIP bytes, SHA256
`c71c1e7fdb705e1b5351ed82d119322c0ff7ff9f9f068d5a75a611aded2935ad`.
The new kernel is 3629632 bytes, SHA256
`17abd1165f3b228457d809adf5f4eb9163c282b6ab5b4828e1c9803a257e5421`.
The new config is 75327 bytes, SHA256
`4ba9caa843f2e00738b1892a3d5cb5ce8e976debdaa3629ce584614bd8f9345d`.

The kernel build alone does not establish browser behavior. The subsequent full
gate above kept OpenFGA `.8` unchanged and directly observed Firefox CRNG
readiness and PostgreSQL startup, but still failed real authorization deadlines.
No successful acceptance or Pages publication is claimed.

## Earlier default-ACPI result and native proof

**Browser acceptance has not passed.** The required result remains two consecutive
genuine `403 → 204 → 200 → 204 → 403` sequences in **each** Chromium and Firefox.
The PR is open; neither a successful native test nor a diagnostic reader
authorizes merging or Pages publication.

The earlier default-ACPI browser gate,
[43464079526](https://github.com/qqrm/openfga-axum-lab/actions/runs/43464079526),
used exact runtime source
[`e381339f`](https://github.com/qqrm/openfga-axum-lab/commit/e381339f80dd76777ef526b0073dc5eb34739f26),
the original ACPI launcher, unchanged emulator and retained application image,
OpenFGA `.8`, and the native-tested bootstrap deny probe. Both engines completed
**zero of two** acceptance passes. Captures were complete and page-error lists empty.

| Browser | Actual result |
| --- | --- |
| Chromium 158.0.8127.18 | PostgreSQL and OpenFGA started. Startup warmup took 10829 ms and the internal gRPC dial 160 ms. The real empty-store bootstrap Check succeeded in 1423 ms; the first original migration tuple Write succeeded in 1583 ms. Nine migration Checks succeeded. The tenth failed with Code 4004 after 3528 ms RPC / 4444 ms HTTP, before Axum readiness. |
| Firefox 159.0 | Linux booted with default ACPI IRQ routing and HPET. Initdb stopped at selecting default max_connections. The final snapshot showed initdb and postgres sleeping with unknown wait targets, guest uptime 983.90 s and 404324 kB available memory. No OpenFGA/probe execution was observed before `LAB\|boot-failed`. |

The Chromium identity is established without logging request bodies. The unchanged
migration iterates sorted users, sorted objects and its fixed action list. Its
first ten observed request lengths match that order exactly. The tenth request is
`user:alice / can_read_assignments / lakekeeper_table:analytics/errors`, with
`HIGHER_CONSISTENCY`. This rule is an eight-branch union in the original policy.
The durations alone do not identify which resolver, datastore or emulation work
caused the timeout.

[Data reader 43466577922](https://github.com/qqrm/openfga-axum-lab/actions/runs/43466577922)
verified the complete Chromium ZIP and selected members, reconstructed the console
from contiguous original JSONL chunks, and exported its exact bytes. Independent
decoding matched 116249 bytes / SHA256
`e4eb761c6c3e9ae4d54f0753004857cefa44b7c01f79fe4cde92e5f125b7b2ab`.
The nine-success/one-failure count excludes all later diagnostic-tail replays.
Those successful RPCs are startup migration work, not acceptance executions.
The [Firefox reader 43467449814](https://github.com/qqrm/openfga-axum-lab/actions/runs/43467449814)
also verified exact console reconstruction. Independent decoding matched 353622 bytes /
SHA256 `0b901d355157c354c559954b7b85fa56ea5c927cafc69b3b9de5d5f19fee0aff`.
Guest uptime and QEMU registration/GC advanced through diagnostics; these facts
neither identify the process wait nor prove a general clock or GC guarantee.

## Native-tested initial deny probe

The browser bootstrap opts into `BROWSER_BOOTSTRAP_EMPTY_STORE_CHECK=yes`.
Absence preserves the ordinary behavior; any other explicit value is rejected
before network activity. Only a freshly created binding takes this path, after
the actual pinned-model readback and empty higher-consistency tuple Read.

The probe makes one actual Check for `user:browser-bootstrap-probe`, relation
`ownership`, object `lakekeeper_table:browser-bootstrap-probe`, using the real
store and model IDs and `HIGHER_CONSISTENCY`. HTTP 200 and the required Boolean
`allowed:false` are mandatory. It writes no tuple, retries no operation and
publishes no configuration on failure, timeout or malformed/allowing response.
The original five-second client/body deadline and three-second OpenFGA deadline
apply. This exercises real model resolution and a direct deny path before Axum
migration; it does not cache an authorization result or remove any migration check.

[Native verification 43456493622](https://github.com/qqrm/openfga-axum-lab/actions/runs/43456493622)
passed on exact integration source `e381339f80dd76777ef526b0073dc5eb34739f26`:
eight Node tests, 25 Rust tests, script/source checks and two original native
acceptance sequences. A real Check response delayed for six seconds failed after
5086 ms with exit 1 and no configuration publication. The positive native probe
used the actual bound IDs and returned a genuine deny. Native performance does
not establish browser performance.

| Producer input or output | Identity |
| --- | --- |
| Bootstrap Rust source | `207d15fab685d3adb9c5d51a048982c2236ae75c` |
| Packaging fixture | `4b7a1e1933083232fb8305a60a47bdd3c32e6689` |
| Bootstrap executable | 7444640 bytes / SHA256 `e1143b7d5f8cc564e09d12c8f685bd7cf33bd0beb1505f2b7cfc1f6f53e1d275` |
| OpenFGA executable | 63066296 bytes / SHA256 `5204b54bed6c9b74afd2517eb82117b1e91befcee8d1c2c709a554d20ad9ee5f` |

The producer's schema-2 retained provenance binds the exact 17 OpenFGA build/test
inputs. It does not claim that every repository file is unchanged. Current build
commands and the required nested bootstrap source checkout are in
[browser-single/README.md](../browser-single/README.md).

## Limits of the tested startup changes

The hardcoded upstream gRPC self-dial is a startup connection, separate from
authorization RPC deadlines. The browser build bounds it at 60 seconds with parent
cancellation and fatal failure intact; upstream request-timeout configuration
does not control this call. Successful dial/model bootstrap now occurs in real
Chromium, but a later authorization timeout still prevents readiness.

The separate `acpi=noirq` candidate in
[43458558432](https://github.com/qqrm/openfga-axum-lab/actions/runs/43458558432)
also produced zero acceptance passes in both engines, stopping before OpenFGA.
The full Firefox console ended initdb at selecting default max_connections;
a final process snapshot showed initdb and postgres sleeping with unknown wait
targets. Chromium stopped producing output and reached the host wait deadline.
These facts do not establish a clock, IRQ or GC cause. The original ACPI routing
is retained.

The original Axum startup computes five independently salted Argon2 password
hashes before authorization migration, and a separate dummy hash before serving.
The retained CLI has no precomputed-hash/seed option. Every acceptance execution
still performs two actual password verifications at `/session`. Neither hash
parameters nor these real checks have been relaxed. No per-hash browser profile
has established that this stage should be changed.

## Checked frontend log coalescing

Source [037a3c22](https://github.com/qqrm/openfga-axum-lab/commit/037a3c22a5fab45a735a6bfea9792c68049a4b62)
changes four files relative to the native-tested e381 source: transcript rendering,
log delivery, regression tests, and three failure-only lines reading process syscall
state within the existing bounded guest diagnostics. Original chunk emission,
sequence/protocol handling, host collector, capture limits and acceptance checker
are preserved. Pending rendering is synchronously flushed before the cutoff;
a render failure remains disqualifying even if every chunk was delivered.

[Transport proof 43470193142](https://github.com/qqrm/openfga-axum-lab/actions/runs/43470193142)
passed 31 Node tests and seven scenarios in each real Chromium 158.0.8127.18 and
Firefox 159.0. Exact chunks, UTF-16 boundaries, JSONL reconstruction, visible tail,
one in-flight delivery, forced partial drain and failure paths were verified.
The synchronous 32256-chunk scenario used 32 host bindings in each engine.
These are transport metrics: the proof starts no VM, downloads no runtime image
and performs no authorization acceptance. It does not establish backend speedup.

Artifact 14060071538 is 46909 ZIP bytes / SHA256
`2845492a3a08a09f4fc957b7907b920362cf521d96545e5569afc9918182cb34`.
Workflow head is 0a8d7dc4ba611fe7071b34005410f4dc832d7b71, attempt 1.
Source hashes and the harness independently matched the reviewed files.

<details>
<summary>Earlier investigation and retained source/build evidence</summary>

Status on 13 November 2026: **the OpenFGA `.8` native proof and versioned
bootstrap producer have passed; browser acceptance remains a separate
required gate**. The completed
retained-runtime [check 43434436745](https://github.com/qqrm/openfga-axum-lab/actions/runs/43434436745)
recorded **zero of two required acceptance passes in both browsers**, with
complete captures and no page errors. Chromium completed model provisioning
but failed the first tuple Write. Firefox reached Axum startup but failed the
application's model readback after a successful RPC. These observations used
OpenFGA `.7`. The required browser result remains two genuine consecutive
sequences per engine; native success does not imply a browser pass. Consult
the current PR's **Browser preview** checks for the browser result; that
production gate remains the publication authority.

The user approved publishing changes and additional checks. The PR has not
been merged and Pages has not been published. The initial proposal below was
prepared against PR #36 head `c527f3ca016b317350a29c0dca38c07e7699230b`;
older observations retain their original source/runtime scope.

## Current model-bootstrap budget and verified producer

The kernel now enables ACPI and `X86_PM_TIMER` alongside the existing IPC and
`HPET_TIMER` settings; its build retains the complete resolved configuration
diff. The launcher removes only the reviewed Linux `acpi=off` token, retains
two sockets/CPUs, 512 MiB and MTTCG, and does not force a clocksource.
The [two-CPU ACPI diagnostic 43424228941](https://github.com/qqrm/openfga-axum-lab/actions/runs/43424228941)
started PostgreSQL and OpenFGA in both browsers but failed initial model
provisioning at the existing five-second client limit. Neither browser reached
an acceptance pass. Its successful diagnostic workflow means observations
were collected, not that the backend passed acceptance.

[Reader 43427570912](https://github.com/qqrm/openfga-axum-lab/actions/runs/43427570912)
validated contiguous captures and exported the full console bytes. Their
verified SHA-256s identify Chromium's outer `Elapsed(())` failure and Firefox's
reqwest `TimedOut` failure. These establish the failing client boundary without
attributing the delay to a particular emulator, database or scheduling cause.

The narrow Rust change adds `BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS`: default
5, strict ASCII integer range 5–30. Only initial model POST and verification
GET use the selected budget, in both reqwest and the enclosing Tokio timeout,
including body consumption. Browser startup selects 30. Health, store
creation/verification and tuple reads remain at five seconds; application
authorization stays at two seconds, OpenFGA requests at three, and original
acceptance requests at five. Migration behavior and limits are unchanged.
Invalid settings, response/URL/schema mismatches, failed model readback or a
nonempty new store fail closed. No uncertain operation is retried or silently
reset; configuration is published atomically only after all checks succeed.

The fourth verified startup payload provides only the initial bootstrap CLI.
The original rootfs still supplies PostgreSQL, Axum, the model, and the absolute
`/usr/local/bin/lab-tool` used by probe and both acceptance executions.
The [preparation instructions](../browser-single/README.md#pages-startup-adaptation-and-acceptance-gate)
use the versioned producer with fixed Rust source `53ff85cfdd3d84c26ae432189790e1e0f72d61db`
and packaging fixture `d7f7ead003c774e30584798adfde38afb854528d`.

[Native run 43433122340](https://github.com/qqrm/openfga-axum-lab/actions/runs/43433122340)
passed all 19 bootstrap Rust tests, both default-five-second failure cases,
opt-in provisioning with each genuine model response delayed for six seconds,
and two actual `403 → 204 → 200 → 204 → 403` acceptance sequences.
The versioned Node implementation then passed its own
[producer verification 43435460160](https://github.com/qqrm/openfga-axum-lab/actions/runs/43435460160).
It passed seven payload tests and the same 19 Rust tests. Default model POST
failed after 5067 ms and verification GET after 5068 ms, each with exit 1,
no published configuration, and the genuine response still withheld. Opt-in
30 completed both real delayed operations, confirmed an empty store and three
actual PostgreSQL model rows, then passed both original native sequences.
The proxy delays complete real responses; it supplies no substitute success.

The producer exports from the exact tested image and passes the production
payload validator. The executable is 7415568 bytes, SHA-256
`d47425c10ebc1d2c1501e86dd7a1f3e8e5b6d2c9a6e4cf8123f20bd5d8c1866f`,
identical to the bootstrap used in browser run 43434436745.
[Artifact 14042363529](https://github.com/qqrm/openfga-axum-lab/actions/runs/43435460160/artifacts/14042363529)
contains the four producer outputs; its ZIP is 3306727 bytes, SHA-256
`cbe04e8e25a102037bd26daaedfe51fdc2f005f477a292e4ffaf7a40193e33e9`.
Source, tested image, input OpenFGA, helper and proof identities are retained
in the build record. This result establishes native producer behavior and
export identity, not browser performance or authorization acceptance.

Chromium [job 131915790998](https://github.com/qqrm/openfga-axum-lab/actions/runs/43434436745/job/131915790998)
completed model POST/GET and two real Read calls. Its first tuple POST,
`/write` with a 3419-byte body, then failed: the RPC observer recorded Code
2000, `context_state=deadline`, `error_kind=model_resolution_deadline` and
about 3300 ms; HTTP recorded 3814 ms. Capture is complete, the page-error list
is empty and no acceptance execution passed. These are guest-observer
durations; they are not a controlled host-time performance comparison.
The original request deadline remains in force. The tested OpenFGA `.7`
binary does not include the new one-shot resolver transfer.

Firefox [job 131915791390](https://github.com/qqrm/openfga-axum-lab/actions/runs/43434436745/job/131915791390)
also completed model provisioning. Its bootstrap model POST took 5209 ms HTTP
and 2052 ms RPC, with a 968 ms decoder cache hit; the verification GET took
5977 ms HTTP and 1077 ms RPC. An empty Read completed in 1062 ms, followed by
`starting real Axum`. The application's separate `ReadAuthorizationModel`
RPC then succeeded in 56 ms, but its HTTP request ended canceled after 5644 ms.
The original application has both a five-second reqwest timeout and an outer
five-second timeout. No acceptance execution passed.

The approximately 5.44 seconds between that RPC completion and HTTP completion
covers gateway protobuf decoding, response JSON encoding, writing and
scheduling. It does not prove that any one function caused the delay.
Bootstrap's opt-in 30 seconds does not apply to this application request and
has not changed its deadline.

## Current .8 candidate and completed native verification

The source at
[501cfc25](https://github.com/qqrm/openfga-axum-lab/commit/501cfc2535f09826550aae97225fa5cbda86f08d)
adds two narrow reuses to the existing browser adaptation. The first privately
retains the fully validated warmup TypeSystem for one matching resolver cache
miss, after the existing accessor has returned the actual model. A single
atomic claim, exact pinned-body comparison, actual nonempty model/store IDs,
private key and cancellation checks preserve ownership. The actual model ID
is installed before the object escapes. Other requests retain their previous
paths; this does not share mutable graphs between independent TypeSystems.

The second can retain one successful, bounded JSON serialization of a real
`ReadAuthorizationModelResponse`. A later hit requires equality of the
entire newly returned protobuf, including its actual model ID and unknown
fields, under the exact intended JSONPb options. The key and returned bytes
are privately copied. Every real RPC/model retrieval still runs, and failures
or unsupported inputs use the original path. The warmup fixture cannot seed
this entry. Existing datastore/model caches may serve an accessor; neither
change promises a physical PostgreSQL query on every RPC.

The encoder records its own duration and cache-hit flag, so a browser trace can
distinguish that work from the complete post-RPC interval. These changes target
observed boundaries; they do not establish their complete cause or guarantee
meeting the existing deadlines. No application authorization, migration,
OpenFGA request or acceptance timeout is increased.

The first native attempt,
[43439983302](https://github.com/qqrm/openfga-axum-lab/actions/runs/43439983302),
failed only `gofmt` on six test-lambda formatting hunks, before Go tests, race
checks or native services ran. The corrected source changes only that formatting.

[Run 43440821229](https://github.com/qqrm/openfga-axum-lab/actions/runs/43440821229)
then completed the actual OpenFGA build and tests, including the new encoder
cases and one-shot transfer. Reported passing test counts were:

| Package | Regular tests | Focused race tests |
| --- | ---: | ---: |
| `cmd/run` | 30 | 30 |
| `pkg/typesystem` | 16 | 16 |
| `pkg/server/commands` | 41 | Not selected |
| `pkg/server` | 4 | 1 |

The optional retained-Rust-wire test was skipped because that optional input
was not supplied. The regular/race gates used the actual patched source.
The output OpenFGA executable is **63066296 bytes**, SHA-256
`5204b54bed6c9b74afd2517eb82117b1e91befcee8d1c2c709a554d20ad9ee5f`;
its `build.json` SHA-256 is
`d7f903c286657d193eddee5b4bf98ccdc2a56431f1924d7b456555fd506d6315`.

That workflow's seven payload tests and 19 Rust tests passed, but the overall
run failed before the backend proof. The service's executable-availability
guard reported `OpenFGA executable is unavailable: /native-openfga/openfga`.
[Data-only reader 43443951581](https://github.com/qqrm/openfga-axum-lab/actions/runs/43443951581)
verified the retained archive and the complete 59-byte stack log, SHA-256
`6374ba75ae3f7089e011e84e0695190d20cf62a878d19b47dc24ca1e3da88d56`.
The ZIP records the executable's mode as 0555; the historical host directory's
permissions were not retained. The evidence establishes the failed
availability guard, not that directory's exact mode.

The [nine-line producer staging correction b267fefd](https://github.com/qqrm/openfga-axum-lab/commit/b267fefdba375395d496e6d89413786c048c70d2)
copies only the already verified binary into an owned mode-0755 directory,
sets the copied executable to 0555 and rechecks its hash. It leaves the
original OpenFGA output untouched and preserves the unprivileged service
user, original availability guard and all runtime/authentication behavior.

[Native verification 43445129977](https://github.com/qqrm/openfga-axum-lab/actions/runs/43445129977)
completed successfully with that correction. It reused the exact retained
Go-tested `.8` executable and its source/build/race evidence; it did not rerun
the Go build. The regression fixture deliberately made the original input
directory 0700 with a 0555 file. Both original modes remained unchanged,
while the verified staged copy was accessible to the service.

The actual producer again passed seven payload tests and 19 Rust tests.
Default model POST failed after **5082 ms** and verification GET after
**5083 ms**, each with exit 1, no configuration publication, and the genuine
response still withheld. With the opt-in 30-second budget, both actual model
responses were delayed for six seconds and then delivered unchanged. The
proof confirmed an empty store and three PostgreSQL model rows, started the
real Axum application, and passed **two consecutive native**
`403 → 204 → 200 → 204 → 403` sequences.

The tested native image is
`sha256:413c5b08d493090a77e5c1c2ed26acf3c2a0148d19372766ca7b0c0499bc6bfd`.
Its exported bootstrap remains 7415568 bytes with the `d47425c1…` SHA-256
recorded above; the production output validator passed.
[Bootstrap artifact 14047378538](https://github.com/qqrm/openfga-axum-lab/actions/runs/43445129977/artifacts/14047378538)
is 3306734 bytes, SHA-256
`f4eb84008b06c01c7457f6ed8c6b2c5675469c726dcaa7eb0f1cfacaa117146e`.
[Evidence artifact 14048022132](https://github.com/qqrm/openfga-axum-lab/actions/runs/43445129977/artifacts/14048022132)
is 23967145 bytes, SHA-256
`d7277d2cbca89d6c522b22f574d20dfc1cecadab64f2bd04480d3e9d61846dca`.

This closes the native regression/provenance gate for the combined candidate;
it does not establish browser latency or browser acceptance. The current PR's
**Browser preview** checks must still verify both real sequences in each
engine, with complete captures and no bypass of ordinary deadlines. See the
[OpenFGA implementation and tests](../browser-single/openfga/README.md).

## Earlier retained result and the PostgreSQL socket change

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

The subsequent `379e3280` change altered **only OpenFGA's PostgreSQL connection host**:
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

At that decoder-correction stage, the original authorization model, Rust
application, bootstrap and acceptance programs were unchanged. Request limits,
startup and emulator budgets, and both required consecutive passes were
preserved. The later bootstrap-only model budget is documented above; all
authorization deadlines and the browser publication requirement remain.

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
but still encountered the Firefox OOM above. That run retained the OpenFGA `.6`
build and request deadlines. A separately tested one-shot TypeSystem transfer
was not included at that stage: that run provided no captured model-resolution
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

## Adaptation retained in this source

- Build OpenFGA from the exact upstream commit with Go 1.24.6. Change
  the gateway startup self-dial budget from three to 60 seconds, observing
  parent cancellation. Preserve `grpc.WithBlock()`, TLS options and fatal
  startup failure. Record source, patch, compiler and executable provenance.
- Before opening listeners, exercise real codecs and fully validate the exact
  pinned model once under one 60-second startup budget. Retain its proven
  conditionless-schema validity and one privately owned, fully validated
  TypeSystem for the first matching resolver cache miss after actual model
  retrieval. Matching model writes still write PostgreSQL; later or unsupported
  resolver calls retain their existing paths. The body comparison excludes
  only the generated ID, and the one claimed TypeSystem receives the actual
  model ID before escape. No tuple or authorization decision is cached.
  Record stage timings. Omit unary raw-payload logging work while retaining
  compact transport metadata and the security middleware.
- Reuse JSON-to-protobuf decoding only for the exact pinned raw model body,
  preparing a private prototype within that same successful startup phase.
  Keep the standard streaming JSON parser, copy the protobuf independently for
  each matching request, and retain default decoding for every other input.
  The generated gateway supplies the real URL store ID; gRPC, validation,
  authentication, deadlines and the actual model write still run. Separately,
  retain at most one exact successful encoding of a real model-read response;
  full response equality, actual ID, private copies and fixed options guard
  reuse after the real RPC. Other responses and errors keep their usual path.
- Supply the guest wrapper, adapted executable, current entrypoint and separate
  initial-bootstrap executable through the existing host mount, with all four
  sizes and checksums verified before VM initialization. Use the
  converter's `mr` instruction for a read-only bind mount. Its `m`
  instruction is writable; see the pinned
  [mount parser](https://github.com/container2wasm/container2wasm/blob/087c313479d51cdf255e4f28ded6ae7d8141297b/cmd/init/main.go).
- Wait for OpenFGA's loopback HTTP listener with a bounded wait and process
  liveness checks, then invoke the verified initial bootstrap once. A listening
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
authorization response is synthesized. Ordinary request deadlines remain:
two seconds for Axum's OpenFGA client, three seconds for OpenFGA requests,
five seconds for application model readback, ordinary bootstrap operations
and the acceptance HTTP client. Only initial bootstrap model POST/verification
GET opt in to 30 seconds; migration behavior and its limits are unchanged.

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

## Earlier 4096-instance browser failure and its next bounded candidate

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

</details>
