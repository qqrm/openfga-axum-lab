# Browser QEMU instance-budget candidate

This directory rebuilds only `out.js` and `qemu-system-x86_64.wasm` from
[QEMU-WASM `8604ed49`](https://github.com/ktock/qemu-wasm/tree/8604ed49a3cde392890b014a8d5a959c8a2fe72a),
using the dependency stages and x86_64 build flags in
[container2wasm `087c313`](https://github.com/container2wasm/container2wasm/blob/087c313479d51cdf255e4f28ded6ae7d8141297b/Dockerfile).
The single C change lowers `MAX_INSTANCE_ALIVE` from 15000 to 4096.
`INSTANTIATE_NUM=1500`, the existing TCI fallback, and GC/eviction logic remain
unchanged. PostgreSQL, OpenFGA, Axum, the authorization model and request
deadlines are outside this patch.

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
4096 is a heuristic chosen to start existing eviction earlier. It is not a byte
budget, an identified Firefox allocator limit, or a demonstrated OOM fix.
GC timing and the performance cost of returning to TCI still require measurement.
This recipe has not yet been built or passed browser acceptance.

## Build and provenance

After authorizing the build, run from the repository:

```sh
bash browser-single/qemu/build.sh
# Optional output directory:
bash browser-single/qemu/build.sh /path/to/emulator-output
```

The script downloads the pinned upstream Dockerfile into a temporary directory
and verifies its SHA256, fetches the exact QEMU commit, verifies both backend
source-file preimages, and checks that the patch changes exactly one line.
A named `qemu-repo` build context supplies the clean checkout. The appended
target independently verifies and applies the patch. Emscripten 4.0.10 is pinned
to the amd64 image digest
`sha256:90b757eb11fa9a0e3ce4d2d9f76d932a56018e4accc37b5a28b2783751e60eb7`.
The build exports only the emulator pair and provenance to
`artifacts/browser-emulator`: file sizes/hashes in `build.json`, source patch,
recipes, compiler version, build configuration, submodule commits and licenses.

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
