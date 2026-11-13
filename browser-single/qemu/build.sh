#!/usr/bin/env bash
# Build only the pinned emulator pair; browser acceptance is a separate gate.
set -Eeuo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(cd -- "$script_dir/../.." && pwd)
if (( $# > 1 )); then
    printf 'usage: %s [OUTPUT_DIR]\n' "$0" >&2
    exit 2
fi
output_dir=$(realpath -m -- "${1:-"$repo_dir/artifacts/browser-emulator"}")
converter_commit=087c313479d51cdf255e4f28ded6ae7d8141297b
converter_dockerfile_sha256=f122e849fcb79d718a73dbef6c6cff6b2b71fd3032c8ea7686dc26c007caa508
qemu_commit=8604ed49a3cde392890b014a8d5a959c8a2fe72a
sdk_version=4.0.10@sha256:90b757eb11fa9a0e3ce4d2d9f76d932a56018e4accc37b5a28b2783751e60eb7

qemu_build_tmp=$(mktemp -d "${TMPDIR:-/tmp}/browser-qemu.XXXXXX")
trap 'rm -rf -- "$qemu_build_tmp"' EXIT
context_dir=$qemu_build_tmp/context
source_dir=$qemu_build_tmp/qemu
mkdir -p "$context_dir" "$source_dir"

curl --fail --silent --show-error --location --retry 3 --max-time 180 \
    "https://raw.githubusercontent.com/container2wasm/container2wasm/$converter_commit/Dockerfile" \
    --output "$context_dir/upstream.Dockerfile"
printf '%s  %s\n' "$converter_dockerfile_sha256" "$context_dir/upstream.Dockerfile" \
    | sha256sum --check -
cp -- "$script_dir/build.sh" "$script_dir/Dockerfile.appendix" \
    "$script_dir/instance-budget.patch" "$script_dir/check-instance-budget.mjs" \
    "$script_dir/check-runtime-diagnostics.mjs" "$script_dir/check-instance-budget-bridge.mjs" \
    "$script_dir/i32-multiply.patch" \
    "$script_dir/check-i32-multiply.mjs" \
    "$script_dir/memory-barrier.patch" "$script_dir/check-memory-barrier.mjs" "$context_dir/"
cat "$context_dir/upstream.Dockerfile" "$context_dir/Dockerfile.appendix" > "$context_dir/Dockerfile"

git -C "$source_dir" init -q
git -C "$source_dir" remote add origin https://github.com/ktock/qemu-wasm.git
git -C "$source_dir" fetch --depth=1 origin "$qemu_commit"
git -C "$source_dir" checkout --detach FETCH_HEAD

check_clean_source() {
    test "$(git -C "$source_dir" rev-parse HEAD)" = "$qemu_commit"
    test "$(git -C "$source_dir" hash-object tcg/wasm32.c)" = d60ce4b5eeb54f4a15b228d57654c52d1cbd1fe6
    test "$(git -C "$source_dir" hash-object tcg/wasm32.h)" = 82ac7e37677a57b27c95e9efe4741a4cec39cf46
    test "$(git -C "$source_dir" hash-object tcg/wasm32/tcg-target.c.inc)" = 3c85588be97b6c53a451e30c0593e8b433ede7b6
    test "$(git -C "$source_dir" hash-object tcg/wasm32/tcg-target.h)" = 37ad4b93c8ab721639e4cd8603a5dbbe69c07105
    git -C "$source_dir" diff --quiet --exit-code HEAD --
}
check_clean_source
# Validate all three exact patches before spending work on dependencies, then supply a
# clean checkout. The Docker target independently validates and applies them.
git -C "$source_dir" apply --unidiff-zero --check --whitespace=error-all "$context_dir/instance-budget.patch"
git -C "$source_dir" apply --unidiff-zero --whitespace=error-all "$context_dir/instance-budget.patch"
git -C "$source_dir" apply --check --whitespace=error-all "$context_dir/i32-multiply.patch"
git -C "$source_dir" apply --whitespace=error-all "$context_dir/i32-multiply.patch"
test "$(git -C "$source_dir" diff --name-only HEAD --)" = $'tcg/wasm32.c\ntcg/wasm32/tcg-target.c.inc'
test "$(git -C "$source_dir" hash-object tcg/wasm32.c)" = e0b26bdc8d6d5b987061f9090ae0b62b76c34e16
test "$(git -C "$source_dir" hash-object tcg/wasm32/tcg-target.c.inc)" = d4078f5adac57d9ccc7d792773fff2ea65a33d6a
grep -qx '#define MAX_INSTANCE_ALIVE 15000' "$source_dir/tcg/wasm32.c"
grep -qx '#define MAX_INSTANCE_RUNNING_LOCAL 2048' "$source_dir/tcg/wasm32.c"
git -C "$source_dir" diff --check HEAD --
node "$script_dir/check-instance-budget.mjs" "$source_dir/tcg/wasm32.c"
node "$script_dir/check-runtime-diagnostics.mjs" "$source_dir/tcg/wasm32.c"
node "$script_dir/check-i32-multiply.mjs" "$source_dir/tcg/wasm32/tcg-target.c.inc"
git -C "$source_dir" apply --check --whitespace=error-all "$context_dir/memory-barrier.patch"
git -C "$source_dir" apply --whitespace=error-all "$context_dir/memory-barrier.patch"
test "$(git -C "$source_dir" hash-object tcg/wasm32/tcg-target.c.inc)" = 4b3e9f8fabcaa3b9a2bb33746f7c1d7046691355
git -C "$source_dir" diff --check HEAD --
node "$script_dir/check-memory-barrier.mjs" "$source_dir/tcg/wasm32/tcg-target.c.inc"
git -C "$source_dir" apply --reverse --whitespace=error-all "$context_dir/memory-barrier.patch"
test "$(git -C "$source_dir" hash-object tcg/wasm32/tcg-target.c.inc)" = d4078f5adac57d9ccc7d792773fff2ea65a33d6a
git -C "$source_dir" apply --reverse --whitespace=error-all "$context_dir/i32-multiply.patch"
git -C "$source_dir" apply --unidiff-zero --reverse --whitespace=error-all "$context_dir/instance-budget.patch"
check_clean_source

docker buildx build --platform linux/amd64 \
    --file "$context_dir/Dockerfile" --target browser-qemu-runtime \
    --build-context "qemu-repo=$source_dir" \
    --build-arg "EMSDK_VERSION_QEMU=$sdk_version" \
    --build-arg EMULATOR_BUILD_JOBS=2 \
    --output "type=local,dest=$qemu_build_tmp/output" "$context_dir"

python3 - "$qemu_build_tmp/output" "$script_dir" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

output, source = map(Path, sys.argv[1:])
metadata = json.loads((output / "build.json").read_text())
expected = {
    "converter_commit": "087c313479d51cdf255e4f28ded6ae7d8141297b",
    "converter_dockerfile_sha256": "f122e849fcb79d718a73dbef6c6cff6b2b71fd3032c8ea7686dc26c007caa508",
    "qemu_commit": "8604ed49a3cde392890b014a8d5a959c8a2fe72a",
    "backend_postimage_blob": "e0b26bdc8d6d5b987061f9090ae0b62b76c34e16",
    "emscripten_version": "4.0.10",
    "emsdk_image": "emscripten/emsdk:4.0.10@sha256:90b757eb11fa9a0e3ce4d2d9f76d932a56018e4accc37b5a28b2783751e60eb7",
    "max_instance_alive_before": 15000,
    "max_instance_alive_after": 15000,
    "max_instance_running_local": 2048,
    "instance_budget_configuration": "shared_c_pre_run_setter",
    "firefox_instance_limit": 8192,
    "chromium_instance_limit": 15000,
    "instance_reservation": "atomic_compare_exchange",
    "instance_budget_tests": "passed",
    "instance_budget_bridge_tests": "passed",
    "runtime_diagnostics": "module_lifecycle_counters_v1",
    "runtime_diagnostics_tests": "passed",
    "i32_multiply_fix": "normalize_operands_before_widening_product",
    "multiply_emitter_preimage_blob": "3c85588be97b6c53a451e30c0593e8b433ede7b6",
    "multiply_emitter_postimage_blob": "d4078f5adac57d9ccc7d792773fff2ea65a33d6a",
    "i32_multiply_tests": "passed",
    "memory_barrier_fix": "emit_atomic_fence_for_tcg_mb",
    "barrier_emitter_preimage_blob": "d4078f5adac57d9ccc7d792773fff2ea65a33d6a",
    "barrier_emitter_postimage_blob": "4b3e9f8fabcaa3b9a2bb33746f7c1d7046691355",
    "memory_barrier_tests": "passed",
    "instantiate_num": 1500,
    "browser_acceptance": "not_run",
}
for key, value in expected.items():
    assert metadata[key] == value, f"Unexpected {key}"
for key, name in [
    ("patch_sha256", "instance-budget.patch"),
    ("builder_sha256", "build.sh"),
    ("dockerfile_appendix_sha256", "Dockerfile.appendix"),
    ("budget_test_sha256", "check-instance-budget.mjs"),
    ("diagnostics_test_sha256", "check-runtime-diagnostics.mjs"),
    ("budget_bridge_test_sha256", "check-instance-budget-bridge.mjs"),
    ("multiply_patch_sha256", "i32-multiply.patch"),
    ("multiply_test_sha256", "check-i32-multiply.mjs"),
    ("barrier_patch_sha256", "memory-barrier.patch"),
    ("barrier_test_sha256", "check-memory-barrier.mjs"),
]:
    assert metadata[key] == hashlib.sha256((source / name).read_bytes()).hexdigest(), key
    assert (output / name).read_bytes() == (source / name).read_bytes(), name
proof_dir = output / "i32-multiply-proof"
proof_bytes = (proof_dir / "summary.json").read_bytes()
assert hashlib.sha256(proof_bytes).hexdigest() == metadata["multiply_proof_sha256"]
proof = json.loads(proof_bytes)
assert proof["qemu_commit"] == metadata["qemu_commit"]
assert proof["source_blob"] == metadata["multiply_emitter_preimage_blob"]
assert proof["candidate_blob"] == metadata["multiply_emitter_postimage_blob"]
assert proof["header_blob"] == "37ad4b93c8ab721639e4cd8603a5dbbe69c07105"
assert proof["patch_sha256"] == metadata["multiply_patch_sha256"]
assert proof["checker_sha256"] == metadata["multiply_test_sha256"]
assert proof["candidate"] == {"total": 96, "passed": 96, "failed": 0}
assert proof["baseline"] == {"total": 96, "passed": 54, "failed": 42}
assert {item.name for item in proof_dir.iterdir()} == set(proof["files"]) | {"summary.json"}
for name, item in proof["files"].items():
    assert Path(name).name == name and name not in (".", "..", "summary.json")
    data = (proof_dir / name).read_bytes()
    assert len(data) == item["bytes"] and hashlib.sha256(data).hexdigest() == item["sha256"], name
barrier_dir = output / "memory-barrier-proof"
barrier_bytes = (barrier_dir / "summary.json").read_bytes()
assert hashlib.sha256(barrier_bytes).hexdigest() == metadata["barrier_proof_sha256"]
barrier = json.loads(barrier_bytes)
assert barrier["status"] == "passed" and barrier["kind"] == "compiled_c_wasm_memory_barrier"
assert barrier["qemu_commit"] == metadata["qemu_commit"]
assert barrier["upstream_blob"] == metadata["multiply_emitter_preimage_blob"]
assert barrier["source_blob"] == metadata["multiply_emitter_postimage_blob"] == metadata["barrier_emitter_preimage_blob"]
assert barrier["candidate_blob"] == metadata["barrier_emitter_postimage_blob"]
assert barrier["header_blob"] == "37ad4b93c8ab721639e4cd8603a5dbbe69c07105"
assert barrier["backend_blob"] == metadata["backend_postimage_blob"]
assert barrier["patch_sha256"] == metadata["barrier_patch_sha256"]
assert barrier["checker_sha256"] == metadata["barrier_test_sha256"]
assert barrier["patch_apply_reverse"] is True and barrier["malformed_fence_controls"] == 3
assert barrier["results"] == [
    {"name": name, "masks": 64, "reqMoCases": 16, "requiredFenceMatches": fences,
     "stackExecutions": 512, "sharedMemoryExecutions": 256, "tciWordsPreserved": True}
    for name, fences in [("baseline", 0), ("candidate", 64)]
]
barrier_files = {
    "baseline-tcg-target.c.inc", "patched-tcg-target.c.inc", "upstream-tcg-target.c.inc",
    "tcg-target.h", "wasm32.c", "tcg-mo.h", "tcg-op.c", "tcg-op-ldst.c", "cpu.h", "atomic.h",
    "memory-barrier.patch", "check-memory-barrier.mjs", "patch-apply-reverse.json",
    "invalid-fe03.wasm", "invalid-fe0301.wasm", "invalid-fe0330.wasm",
} | {
    f"{variant}-{suffix}" for variant in ["baseline", "candidate"]
    for suffix in ["emitter.c", "compiler.json", "emitted.txt", "no-memory-stack.wasm",
                   "shared-stack.wasm", "shared-write-fence-read.wasm"]
}
assert set(barrier["files"]) == barrier_files
assert {item.name for item in barrier_dir.iterdir()} == barrier_files | {"summary.json"}
for name, item in barrier["files"].items():
    data = (barrier_dir / name).read_bytes()
    assert len(data) == item["bytes"] and hashlib.sha256(data).hexdigest() == item["sha256"], name
bridge_dir = output / "instance-budget-bridge-proof"
bridge_bytes = (bridge_dir / "summary.json").read_bytes()
assert hashlib.sha256(bridge_bytes).hexdigest() == metadata["budget_bridge_proof_sha256"]
bridge = json.loads(bridge_bytes)
assert bridge["kind"] == "compiled_emscripten_budget_bridge"
assert bridge["emscripten_version"] == metadata["emscripten_version"]
assert bridge["source_blob"] == metadata["backend_postimage_blob"]
assert bridge["checker_sha256"] == metadata["budget_bridge_test_sha256"]
assert bridge["candidate"] == {"total": 2, "passed": 2}
assert bridge["negative_controls"] == {"total": 2, "rejected": 2}
assert {item.name for item in bridge_dir.iterdir()} == set(bridge["files"]) | {"summary.json"}
for name, item in bridge["files"].items():
    assert Path(name).name == name and name not in (".", "..", "summary.json")
    data = (bridge_dir / name).read_bytes()
    assert len(data) == item["bytes"] and hashlib.sha256(data).hexdigest() == item["sha256"], name
assert set(metadata["files"]) == {"out.js", "qemu-system-x86_64.wasm"}
for name, expected_file in metadata["files"].items():
    data = (output / name).read_bytes()
    assert data
    assert len(data) == expected_file["bytes"], name
    assert hashlib.sha256(data).hexdigest() == expected_file["sha256"], name
assert (output / "qemu-system-x86_64.wasm").read_bytes()[:8] == b"\0asm\x01\0\0\0"
assert "qemu-system-x86_64.wasm" in (output / "out.js").read_text()
assert not any((output / name).exists() for name in [
    "qemu-system-x86_64.data", "load.js", "arg-module.js", "rootfs.bin", "bzImage",
])
PY
mkdir -p "$output_dir"
cp -a -- "$qemu_build_tmp/output/." "$output_dir/"
printf 'Verified emulator candidate prepared in %s; browser acceptance has not run.\n' "$output_dir"
