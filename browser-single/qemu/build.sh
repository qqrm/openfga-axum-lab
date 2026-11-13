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
    "$script_dir/instance-budget.patch" "$script_dir/check-instance-budget.py" "$context_dir/"
cat "$context_dir/upstream.Dockerfile" "$context_dir/Dockerfile.appendix" > "$context_dir/Dockerfile"

git -C "$source_dir" init -q
git -C "$source_dir" remote add origin https://github.com/ktock/qemu-wasm.git
git -C "$source_dir" fetch --depth=1 origin "$qemu_commit"
git -C "$source_dir" checkout --detach FETCH_HEAD

check_clean_source() {
    test "$(git -C "$source_dir" rev-parse HEAD)" = "$qemu_commit"
    test "$(git -C "$source_dir" hash-object tcg/wasm32.c)" = d60ce4b5eeb54f4a15b228d57654c52d1cbd1fe6
    test "$(git -C "$source_dir" hash-object tcg/wasm32.h)" = 82ac7e37677a57b27c95e9efe4741a4cec39cf46
    git -C "$source_dir" diff --quiet --exit-code HEAD --
}
check_clean_source
# Validate the exact edit before spending work on dependencies, then supply a
# clean checkout. The Docker target independently validates and applies it.
git -C "$source_dir" apply --unidiff-zero --check --whitespace=error-all "$context_dir/instance-budget.patch"
git -C "$source_dir" apply --unidiff-zero --whitespace=error-all "$context_dir/instance-budget.patch"
test "$(git -C "$source_dir" diff --name-only HEAD --)" = tcg/wasm32.c
test "$(git -C "$source_dir" hash-object tcg/wasm32.c)" = 207a59b01ee8655fb542173fd944549653c3d29c
grep -qx '#define MAX_INSTANCE_ALIVE 8192' "$source_dir/tcg/wasm32.c"
grep -qx '#define MAX_INSTANCE_RUNNING_LOCAL 2048' "$source_dir/tcg/wasm32.c"
git -C "$source_dir" diff --check HEAD --
python3 "$script_dir/check-instance-budget.py" "$source_dir/tcg/wasm32.c"
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
    "backend_postimage_blob": "207a59b01ee8655fb542173fd944549653c3d29c",
    "emscripten_version": "4.0.10",
    "emsdk_image": "emscripten/emsdk:4.0.10@sha256:90b757eb11fa9a0e3ce4d2d9f76d932a56018e4accc37b5a28b2783751e60eb7",
    "max_instance_alive_before": 15000,
    "max_instance_alive_after": 8192,
    "max_instance_running_local": 2048,
    "instance_reservation": "atomic_compare_exchange",
    "instance_budget_tests": "passed",
    "instantiate_num": 1500,
    "browser_acceptance": "not_run",
}
for key, value in expected.items():
    assert metadata[key] == value, f"Unexpected {key}"
for key, name in [
    ("patch_sha256", "instance-budget.patch"),
    ("builder_sha256", "build.sh"),
    ("dockerfile_appendix_sha256", "Dockerfile.appendix"),
    ("budget_test_sha256", "check-instance-budget.py"),
]:
    assert metadata[key] == hashlib.sha256((source / name).read_bytes()).hexdigest(), key
    assert (output / name).read_bytes() == (source / name).read_bytes(), name
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
