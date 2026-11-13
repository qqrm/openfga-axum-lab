# Browser QEMU adaptations

This directory rebuilds only `out.js` and `qemu-system-x86_64.wasm` from
[QEMU-WASM `8604ed49`](https://github.com/ktock/qemu-wasm/tree/8604ed49a3cde392890b014a8d5a959c8a2fe72a),
using the dependency stages and x86_64 build flags in
[container2wasm `087c313`](https://github.com/container2wasm/container2wasm/blob/087c313479d51cdf255e4f28ded6ae7d8141297b/Dockerfile).
The instance-budget patch separates the retained-function threshold from the total number of
live instances. At 2048 retained instances on a CPU, it detaches the oldest
half before another compilation. The page selects a global backstop of
8192 live instances for Firefox and the upstream 15000 for Chromium,
including detached instances awaiting a browser GC callback. An atomic
compare/exchange reserves a slot before compilation, so simultaneous CPU
workers cannot exceed the selected count. `INSTANTIATE_NUM=1500`, `MAX_EXEC_NUM=50000`
and the TCI fallback remain unchanged. PostgreSQL, OpenFGA, Axum, the
authorization model and request deadlines are outside this patch.

A separate eight-line patch fixes signed and unsigned 32-bit widening
multiplication in the Wasm emitter. It preserves the retention, diagnostic and
execution rules above. This is an arithmetic correctness fix, not a demonstrated
remedy for browser OOM or startup timeouts.

## Startup budget interface

`Module._qemu_wasm_set_instance_limit(limit)` is an explicit C export provided
by `EMSCRIPTEN_KEEPALIVE`. It accepts only 8192 and 15000, returning that value
on success or zero on refusal. The launcher calls it synchronously in `preRun`
and requires the exact return value before starting the emulator. No copied
JavaScript `Module` property or worker-local configuration is used for admission.

The C state is shared by all workers. Its sign records whether configuration
is frozen; compare/exchange linearizes a setter racing the first vCPU's
`init_wasm32`. After that initialization starts, every setter is refused,
including when the live count later returns to zero. The caller's `preRun`
boundary is before `main`; the C refusal boundary is the first vCPU
initialization, before any translated-block dispatch. The default and maximum
remain 15000. Both admission and the existing budget-induced yield condition
read the same frozen limit. The ring still has 15000 slots, eviction still
starts at 2048 retained instances per CPU, and `live_limit` is included in
the existing numeric diagnostic snapshots.

This is a resource-budget candidate. It does not catch or retry OOM, force GC,
alter an authorization deadline, or change the real-service acceptance criteria.

## Multiplication correctness

The pinned `tcg_wasm_out_mulu2_i32` and `tcg_wasm_out_muls2_i32` multiply raw
64-bit register globals. TCG's i32 operands require only their low 32 bits:
unsigned operands must be zero-extended, and signed operands sign-extended,
before the widening multiply. For example, signed `0xffffffff × 2` with clean
upper bits must produce high word `0xffffffff` and low word `0xfffffffe`;
the upstream emitter produces high word `0x00000001` instead.

`i32-multiply.patch` adds only `i32.wrap_i64` followed by the appropriate
`i64.extend_i32_u/s` for each operand. These are the two multiplication hunks
from [upstream PR #54](https://github.com/ktock/qemu-wasm/pull/54); no other
proposed i32 or POPCNT changes are included. Ordinary guest 32-bit MUL/IMUL
can reach these emitters without an optional CPUID feature.
The emitter file's Git blob changes from
`3c85588be97b6c53a451e30c0593e8b433ede7b6` to
`d4078f5adac57d9ccc7d792773fff2ea65a33d6a`.

`check-i32-multiply.mjs` verifies the actual patched file and header, reconstructs
and verifies the pinned upstream preimage, then extracts the real C emitters,
instruction helpers, register mapping and LEB encoder from both versions. A
small native output-sink adapter records their bytes; Node executes those bytes
as real Wasm with mutable i64 globals. BigInt references check 96 cases per
version: signed/unsigned bounds, clean and polluted upper bits, and distinct,
aliased, swapped and shared input registers. The upstream negative control
produces 42 mismatches, including the clean-upper signed example; the candidate
passes all 96. Other registers must remain unchanged. This checks the actual
emitted arithmetic, not the full guest translator or a browser VM.

The build retains full pinned/postimage sources, extracted C, emitted bytes,
tiny Wasm modules, compiler logs and per-case results in `i32-multiply-proof/`.
Its hashed summary lists every proof file. This archive is separate from the
required two-pass browser acceptance gate.

## Evidence and limits

A local Firefox 159 diagnostic run on 2026-11-12 failed in
`new WebAssembly.Module` while compiling a 22947-byte TB module. Core 0 had
registered 6250 instances; the latest core 1 snapshot recorded 4864, giving a
lower bound of 11114 total. Both recorded zero detached instances and zero
FinalizationRegistry callbacks. The UI remained unready/unaccepted and recorded
`out of memory`; the run ended after about 119 seconds.
The retained evidence is `firefox-module-probe/firefox-console.txt`, SHA256
`092b86fbcf688e5269b5b132ccb670493b7c19cc1ae4122fbb6d39a6679d898b`.

The pinned backend starts eviction at its instance-count limit; ordinary
[TB flush/region reset](https://github.com/ktock/qemu-wasm/blob/8604ed49a3cde392890b014a8d5a959c8a2fe72a/tcg/region.c)
does not release the separately retained
[compiled WASM functions](https://github.com/ktock/qemu-wasm/blob/8604ed49a3cde392890b014a8d5a959c8a2fe72a/tcg/wasm32.c).
The first candidate lowered the same threshold to 4096. Its actual build passed,
but [browser acceptance failed](https://github.com/qqrm/openfga-axum-lab/actions/runs/43330935595):
Chromium timed out during PostgreSQL initialization; Firefox reached OpenFGA but
exhausted the unchanged 60-second warmup budget during the second cycle.
Neither completed an acceptance pass. No Firefox OOM was recorded in that run;
that does not establish a general memory bound.

Two bounded local Chromium 157 diagnostics used that actual 4096 WASM with
numeric JS counters. At the first limit, compilation stopped with 4096
registered instances and roughly half detached. FinalizationRegistry callbacks
arrived only later, after which compilation resumed. The second probe counted
actual `emscripten_sleep(0)` callbacks inside Asyncify: hundreds occurred while
no GC callback had arrived. Its first observed callback was about 30 seconds
after detachment, with five-second sampling. Increasing yield frequency is not
supported by that observation. These probes measured counters, not native code
bytes, and were not production acceptance runs.

The intermediate 8192/2048 candidate still failed both browsers in
[run 43343577131](https://github.com/qqrm/openfga-axum-lab/actions/runs/43343577131).
A longer local numeric diagnostic then observed the entire OpenFGA warmup at
8192 live instances, with 5120 detached pending GC and 3072 retained. No new
compilations occurred during that warmup, despite thousands of actual zero-sleep
callbacks. The containing sampled plateau lasted at least 296.024 seconds.
The console SHA-256 is
`9be5ad78700239f00519fdd21a8b5cb80ad785304d8d2d67ffc271f8a615cdd2`.
Console-window replay was deduplicated; these are counters, not native-memory
measurements or uninstrumented CI performance results.

The previous candidate restored the original 15000 global backstop while retaining
early detachment at 2048 per CPU and strict atomic admission. Relative to the
8192 C postimage, only this numerical backstop changes. This provides more room
between detachment and the hard fallback limit. It does not revert to the old
late-eviction behavior that reached Firefox OOM without any detached instances.

The revised candidate detaches functions while allocation headroom remains.
Detachment does **not** decrement the live count; only a delivered GC callback
does. A pending GC batch no longer prevents the next eviction once the retained
count reaches 2048 again. Repeated checks below that threshold do nothing, so a
full live budget cannot repeatedly drain the hot cache. A CPU can retain at
most 2048 functions, and the two-CPU guest at most 4096 in total; the unused
quota of one CPU is not shared with the other. The 15000-slot FIFO is reused
only after entries have been detached.

Both thresholds are heuristics. The strict count is not a byte budget or a
guarantee against browser OOM, and browser GC still has no scheduling guarantee.
Early eviction may cause recompilation; a full live budget still falls back to
TCI. The actual 15000/2048 build passed its source/accounting checks, but
[run 43353568819](https://github.com/qqrm/openfga-axum-lab/actions/runs/43353568819)
still failed both browsers. Chromium wrote the real model and started Axum,
then hit the unchanged OpenFGA deadline on its first tuple Write during SQL
authorization migration. Firefox failed with `out of memory` in
`new WebAssembly.Module` during PostgreSQL initialization, after 113.537 seconds.
Neither completed an acceptance pass. Firefox's stack includes earlier
Asyncify sleep/resume frames; it does not support claiming that the worker never
yielded. That CI trace has no instance/GC counters, so their state at OOM is
unknown. The final evidence ZIP SHA-256 is
`caa925fa4dfe8e1d130e25213ec228749d121b6c27dfeb7762538ccbff915780`.

[Run 43358719645](https://github.com/qqrm/openfga-axum-lab/actions/runs/43358719645)
used the same actual emulator pair and verified service/client affinity to CPU 0.
Firefox again failed in `new WebAssembly.Module` during PostgreSQL initialization,
after 119.996 seconds. Chromium reached its first tuple Write, which returned
OpenFGA code 2000 / HTTP 400; neither browser completed acceptance. The final
evidence ZIP SHA-256 is
`339bad2536bfdd6d88ec4246fcafc6929366958bbb54e354912c4fc9c91f90df`.
Pinning did not eliminate the observed Firefox failure and is removed from the
guest wrapper. This is not a controlled comparison of CPU scheduling policies.

The diagnostic build retained those admission, eviction and execution rules and added
passive per-worker counts for compilation, registration, detachment and delivered
GC callbacks. Sparse snapshots and a failure-phase snapshot identify the
remaining lifecycle pressure; an exception is rethrown unchanged. The counters
retain no modules, instances or exported functions, and introduce no extra
yield, timer or GC request. Encoded input bytes are not native executable-memory
measurements. Other workers' last snapshots can be stale: cumulative counts
give lower bounds, but live/pending counts from different times cannot be summed
into a current global count. No browser flags, forced GC or synthetic
authorization results are introduced.
`registered_total` counts completed `addFunction` operations, not merely the
earlier finalizer registration. A failed construction can leave those phase
counts different. The diagnostic C postimage is
`ccbd0ab09ada98440cda78456cdb2966443ec05d`.

The next actual [run 43373341199](https://github.com/qqrm/openfga-axum-lab/actions/runs/43373341199)
captured the full Firefox stream and failed in `new WebAssembly.Module` during
PostgreSQL initialization after 134.481 seconds. At the failure, core 1 had
registered 94911 instances, detached 93184, and received 86016 GC callbacks:
1727 retained plus 7168 detached pending callbacks, or 8895 not yet finalized
registrations on that worker alone. The failed attempt had also reserved its
own global slot. Thus a global cap of 8192 would have refused that admission
under the same preceding history. The latest core 0 snapshot, 121 ms earlier,
showed 2048 retained and no pending callbacks; these snapshots are not a
simultaneous global memory measurement. Both eviction and delivered GC were
active. Chromium's capture overflowed on one-character serial chunks and did
not provide an acceptance result in that run.

The current candidate therefore selects 8192 only for Firefox and retains
15000 for Chromium. The earlier Firefox 8192 run reached OpenFGA and completed
two warmup cycles before its former four-cycle warmup exhausted 60 seconds.
The current one-cycle/schema-cache OpenFGA build has not completed browser
acceptance with this selected limit. A lower limit may reduce memory pressure
but increase TCI execution and startup latency; it guarantees neither GC timing
nor freedom from OOM. The new C postimage is
`e0b26bdc8d6d5b987061f9090ae0b62b76c34e16`.

## Build and provenance

After authorizing the build, run from the repository:

```sh
bash browser-single/qemu/build.sh
# Optional output directory:
bash browser-single/qemu/build.sh /path/to/emulator-output
```

The script downloads the pinned upstream Dockerfile into a temporary directory
and verifies its SHA256, fetches the exact QEMU commit, verifies the backend and
emitter/header preimages, and checks both exact patched C postimages. Only
`tcg/wasm32.c` and `tcg/wasm32/tcg-target.c.inc` may change.
A named `qemu-repo` build context supplies the clean checkout. The appended
target independently verifies and applies both patches. Focused native tests
extract the actual patched accounting/eviction functions, exercise competing
reservations at both limits, configuration/freeze races, the actual yield
condition, delayed GC accounting and FIFO eviction, and retain
their result. They use native atomic/JS adapters and do not claim to execute
the WASM engine or browser GC. Separate tests extract the actual JavaScript
bodies, compile a real small WASM module, and check counter updates, error
identity, failure phases and logger failure. Controlled finalizer callbacks in
these tests do not establish browser GC timing or memory use.
`check-instance-budget-bridge.mjs` additionally compiles the extracted C with
SDK 4.0.10 and executes the generated ES-module glue. The actual `preRun`
setter must be visible to `PROXY_TO_PTHREAD` main and two further pthreads;
both limits, shared admission, invalid input and late calls are checked.
Compiled negative controls replace the shared state with thread-local state
and remove the export, and must fail. Raw C, glue, Wasm, logs and a hashed
summary are retained in `instance-budget-bridge-proof/`. The fixture uses the
production threading/Asyncify/ES-module settings with a 64 MiB heap and
`EXIT_RUNTIME=1`; it is not a complete QEMU or browser workload. The SDK build
target requires this proof before compiling the full emulator.
The multiplication checker runs in both host preflight and the build target;
an unchanged upstream emitter cannot pass as the candidate.
Emscripten 4.0.10 is pinned
to the amd64 image digest
`sha256:90b757eb11fa9a0e3ce4d2d9f76d932a56018e4accc37b5a28b2783751e60eb7`.
The build exports only the emulator pair and provenance to
`artifacts/browser-emulator`: file sizes/hashes in `build.json`, source patches,
recipes, accounting, diagnostic and multiplication tests/results, compiler version, build configuration,
submodule commits and licenses.

The upstream dependency stages still use package repositories and network
downloads; the compiler digest alone does not guarantee bit-for-bit rebuilds.
Preserving the dependency-stage OCI image by digest can freeze that environment.

Integration must first verify the original runtime ZIP and perform the existing
SYSVIPC kernel repack, then verify and overlay just this emulator pair.
Current data/loader/argument/kernel hashes must remain unchanged by the overlay;
the original runtime manifest continues to describe the original artifact.
Emulator provenance belongs in additive derived/startup metadata.
Both Chromium and Firefox must independently complete two genuine consecutive
`403 → 204 → 200 → 204 → 403` passes before publication can be considered.
