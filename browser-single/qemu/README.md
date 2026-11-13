# Browser QEMU instance-budget candidate

This directory rebuilds only `out.js` and `qemu-system-x86_64.wasm` from
[QEMU-WASM `8604ed49`](https://github.com/ktock/qemu-wasm/tree/8604ed49a3cde392890b014a8d5a959c8a2fe72a),
using the dependency stages and x86_64 build flags in
[container2wasm `087c313`](https://github.com/container2wasm/container2wasm/blob/087c313479d51cdf255e4f28ded6ae7d8141297b/Dockerfile).
The C patch separates the retained-function threshold from the total number of
live instances. At 2048 retained instances on a CPU, it detaches the oldest
half before another compilation. The global backstop is the upstream value of
15000 live instances,
including detached instances awaiting a browser GC callback. An atomic
compare/exchange reserves a slot before compilation, so simultaneous CPU
workers cannot exceed that count. `INSTANTIATE_NUM=1500`, `MAX_EXEC_NUM=50000`
and the TCI fallback remain unchanged. PostgreSQL, OpenFGA, Axum, the
authorization model and request deadlines are outside this patch.

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

The current candidate restores the original 15000 global backstop while retaining
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
TCI. This revised candidate needs a fresh real build and acceptance in both
browsers. No browser flags, forced GC or synthetic authorization results are
part of the candidate.

## Build and provenance

After authorizing the build, run from the repository:

```sh
bash browser-single/qemu/build.sh
# Optional output directory:
bash browser-single/qemu/build.sh /path/to/emulator-output
```

The script downloads the pinned upstream Dockerfile into a temporary directory
and verifies its SHA256, fetches the exact QEMU commit, verifies both backend
source-file preimages, and checks the exact patched C postimage. Only
`tcg/wasm32.c` may change.
A named `qemu-repo` build context supplies the clean checkout. The appended
target independently verifies and applies the patch. Focused native tests
extract the actual patched accounting/eviction functions, exercise competing
reservations at the limit, delayed GC accounting and FIFO eviction, and retain
their result. They use native atomic/JS adapters and do not claim to execute
the WASM engine or browser GC. Emscripten 4.0.10 is pinned
to the amd64 image digest
`sha256:90b757eb11fa9a0e3ce4d2d9f76d932a56018e4accc37b5a28b2783751e60eb7`.
The build exports only the emulator pair and provenance to
`artifacts/browser-emulator`: file sizes/hashes in `build.json`, source patch,
recipes, accounting tests/results, compiler version, build configuration,
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
