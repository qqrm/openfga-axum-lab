#!/usr/bin/env bash
# Produce only the initial-bootstrap overlay from a tested fixed native image.
set -euo pipefail
umask 022

repo_root=$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
proof_dir="$repo_root/artifacts/browser-bootstrap-proof"
output_dir="$repo_root/artifacts/browser-bootstrap"
bootstrap_run_id=${GITHUB_RUN_ID:-local-$$}
bootstrap_workdir=

cleanup_owned() {
    local container owner
    for container in native-bootstrap-proxy native-bootstrap-stack native-bootstrap-export native-bootstrap-rustfmt native-bootstrap-rust-tests; do
        owner=$(docker inspect -f '{{index .Config.Labels "io.authzlab.native-bootstrap-run"}}' "$container" 2>/dev/null || true)
        if [[ "$owner" != "$bootstrap_run_id" ]]; then continue; fi
        docker logs "$container" >"$proof_dir/$container-cleanup.log" 2>&1 || true
        if [[ "$container" == native-bootstrap-stack ]]; then
            docker cp "$container:/tmp/browser-lab/initdb.log" "$proof_dir/initdb.log" || true
            docker cp "$container:/tmp/browser-lab/postgres.log" "$proof_dir/postgres.log" || true
        fi
        docker rm -f "$container" || true
    done
}

if [[ "${1:-}" == --cleanup ]]; then
    if [[ $# -ne 1 || -z "${GITHUB_RUN_ID:-}" ]]; then
        echo "Cleanup requires the current GitHub run identity." >&2
        exit 64
    fi
    mkdir -p "$proof_dir"
    cleanup_owned
    exit 0
fi
if [[ $# -ne 2 ]]; then
    echo "Usage: $0 FIXED_CORE_CHECKOUT FIXED_PACKAGING_FIXTURE" >&2
    exit 64
fi
core_dir=$(realpath -- "$1")
fixture_dir=$(realpath -- "$2")
core_commit=53ff85cfdd3d84c26ae432189790e1e0f72d61db
fixture_commit=d7f7ead003c774e30584798adfde38afb854528d
test "$(git -C "$core_dir" rev-parse HEAD)" = "$core_commit"
test "$(git -C "$fixture_dir" rev-parse HEAD)" = "$fixture_commit"
git -C "$core_dir" diff --exit-code HEAD --
git -C "$fixture_dir" diff --exit-code HEAD --
test "$(git -C "$core_dir" hash-object tools/bootstrap.rs)" = a38ba1e8f792e7c0ed019c92f0ffd08a5d74a7a3
test "$(git -C "$core_dir" hash-object tools/bootstrap_tests.rs)" = f5b5e56b04f9d8dd518228f84b7470b9949c65ca
for relative in tools/bootstrap.rs tools/bootstrap_tests.rs Cargo.lock browser-single/Dockerfile fga/model.json; do
    cmp "$core_dir/$relative" "$repo_root/$relative"
    cmp "$core_dir/$relative" "$fixture_dir/$relative"
done
for relative in browser-single/entrypoint.sh browser-single/pages/guest.sh \
                browser-single/pages/startup-payload.mjs browser-single/startup-payload.test.mjs \
                browser-single/prepare-pages.sh browser-single/check-pages.mjs; do
    cmp "$fixture_dir/$relative" "$repo_root/$relative"
done
test ! -e "$output_dir"
test ! -e "$proof_dir"
mkdir -p "$proof_dir"
chmod 0755 "$proof_dir"
for container in native-bootstrap-proxy native-bootstrap-stack native-bootstrap-export native-bootstrap-rustfmt native-bootstrap-rust-tests; do
    if docker inspect "$container" >/dev/null 2>&1; then
        echo "Refusing to reuse an existing container: $container" >&2
        exit 65
    fi
done

# The reviewed proof keeps its original relative paths in an isolated workspace.
bootstrap_workdir=$(mktemp -d)
# ShellCheck versions differ on detecting EXIT/INT/TERM callbacks.
# shellcheck disable=SC2317,SC2329
cleanup() {
    local bootstrap_exit_status=$?
    trap - EXIT INT TERM
    cleanup_owned
    rm -rf -- "$bootstrap_workdir"
    exit "$bootstrap_exit_status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir "$bootstrap_workdir/artifacts"
ln -s "$core_dir" "$bootstrap_workdir/source"
ln -s "$fixture_dir" "$bootstrap_workdir/packaging"
ln -s "$proof_dir" "$bootstrap_workdir/artifacts/native-bootstrap"
ln -s "$repo_root/artifacts/browser-openfga" "$bootstrap_workdir/artifacts/native-bootstrap-input"
cp "$repo_root/browser-single/bootstrap/native-proof.py" "$proof_dir/native-proof.py"
chmod 0644 "$proof_dir/native-proof.py"
export GITHUB_RUN_ID="$bootstrap_run_id"
export GITHUB_RUN_ATTEMPT=${GITHUB_RUN_ATTEMPT:-1}
if [[ -z "${GITHUB_SHA:-}" ]]; then
    GITHUB_SHA=$(git -C "$repo_root" rev-parse HEAD)
fi
export GITHUB_SHA

# Consume the already built and regression-tested OpenFGA artifact. No download,
# fallback executable, OpenFGA build or emulator build occurs in this producer.
python3 - "$repo_root" "$proof_dir" <<'PY'
import hashlib, json, re, sys
from pathlib import Path
repo, evidence = map(Path, sys.argv[1:])
directory = repo / "artifacts/browser-openfga"
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
def require(ok, message):
    if not ok:
        raise SystemExit(message)
build = json.loads((directory / "build.json").read_text())
binary = directory / "openfga"
require(binary.is_file() and binary.stat().st_mode & 0o111, "OpenFGA executable is unavailable")
actual = digest(binary)
require(re.fullmatch(r"[0-9a-f]{64}", build.get("binary_sha256", "")) and
        actual == build["binary_sha256"], "OpenFGA metadata/binary identity differs")
for key, value in {
    "source_commit": "293893b22a53d8a130a5f7c97cab4bc8e91bcaa4",
    "upstream_version": "1.10.3", "startup_dial_timeout_seconds": 60,
    "authorization_request_timeout_default_seconds": 3,
    "warmup_timeout_seconds": 60, "warmup_cycles": 1,
    "startup_dial_tests": "passed", "warmup_tests": "passed",
    "model_codec_tests": "passed", "model_validation_tests": "passed",
    "write_typesystem_tests": "passed",
    "warmup_model_sha256": "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d"
}.items():
    require(build.get(key) == value, "Unexpected verified OpenFGA field: " + key)
for key, name in {
    "patch_sha256": "startup-dial.patch",
    "helper_sha256": "browser_http_dial.go",
    "tests_sha256": "browser_http_dial_test.go",
    "warmup_helper_sha256": "browser_http_warmup.go",
    "warmup_tests_sha256": "browser_http_warmup_test.go",
    "model_codec_helper_sha256": "browser_model_codec.go",
    "model_codec_tests_sha256": "browser_model_codec_test.go",
    "model_codec_integration_tests_sha256": "browser_model_codec_integration_test.go",
    "model_validation_helper_sha256": "browser_model_validation.go",
    "model_validation_tests_sha256": "browser_model_validation_test.go",
    "write_typesystem_tests_sha256": "browser_write_typesystem_test.go"
}.items():
    require(build.get(key) == digest(repo / "browser-single/openfga" / name),
            "OpenFGA source/proof identity differs: " + key)
require(digest(repo / "fga/model.json") == build["warmup_model_sha256"], "Pinned model changed")
(evidence / "openfga.sha256").write_text(actual + "\n")
(evidence / "openfga-build.json").write_text(json.dumps(build, indent=2) + "\n")
print(json.dumps({"scope": "reuse verified OpenFGA build", "binary_sha256": actual,
                  "binary_bytes": binary.stat().st_size, "source_commit": build["source_commit"]}))
PY
IFS= read -r openfga_sha <"$proof_dir/openfga.sha256"
python3 -m py_compile "$proof_dir/native-proof.py"
{
    node --check "$repo_root/browser-single/pages/startup-payload.mjs"
    node --check "$repo_root/browser-single/startup-payload.test.mjs"
    node --check "$repo_root/browser-single/check-pages.mjs"
    node --test "$repo_root/browser-single/startup-payload.test.mjs"
    for script in "$repo_root/browser-single/entrypoint.sh" \
                  "$repo_root/browser-single/pages/guest.sh" \
                  "$repo_root/browser-single/prepare-pages.sh" \
                  "$repo_root/browser-single/bootstrap/build.sh"; do
        bash -n "$script"
        shellcheck "$script"
    done
} 2>&1 | tee "$proof_dir/wrapper-tests.log"

timeout --kill-after=15s 300s docker run --rm --name native-bootstrap-rustfmt \
    --label "io.authzlab.native-bootstrap-run=$bootstrap_run_id" \
    --mount "type=bind,src=$core_dir,dst=/src,readonly" \
    --workdir /src rust:1.99.1-slim-bookworm bash -euc \
    'rustup component add rustfmt; rustfmt --edition 2024 --check tools/bootstrap.rs tools/bootstrap_tests.rs' \
    2>&1 | tee "$proof_dir/rustfmt.log"
git -C "$core_dir" diff --exit-code HEAD --

timeout --kill-after=15s 1500s docker buildx build --load --platform linux/amd64 --progress plain \
    --file "$core_dir/browser-single/Dockerfile" --iidfile "$proof_dir/image-id.txt" \
    "$core_dir" 2>&1 | tee "$proof_dir/image-build.log"
bootstrap_image=$(<"$proof_dir/image-id.txt")
test "$(docker image inspect "$bootstrap_image" --format '{{.Id}}')" = "$bootstrap_image"

# Reuse identical cached build layers for the tests; no second service image.
timeout --kill-after=15s 900s docker buildx build --load --platform linux/amd64 --target rust-build \
    --file "$core_dir/browser-single/Dockerfile" --iidfile "$proof_dir/test-image-id.txt" \
    "$core_dir" 2>&1 | tee "$proof_dir/test-stage.log"
test_image=$(<"$proof_dir/test-image-id.txt")
timeout --kill-after=15s 900s docker run --rm --name native-bootstrap-rust-tests \
    --label "io.authzlab.native-bootstrap-run=$bootstrap_run_id" "$test_image" cargo test --release --locked \
    --no-default-features --features http --bin lab-tool bootstrap::tests -- --nocapture \
    2>&1 | tee "$proof_dir/unit-tests.log"
grep -Fq 'test result: ok. 19 passed; 0 failed;' "$proof_dir/unit-tests.log"

(
    cd "$bootstrap_workdir"
    BROWSER_BOOTSTRAP_OPENFGA_SHA256="$openfga_sha" \
        timeout --kill-after=15s 720s python3 artifacts/native-bootstrap/native-proof.py
) 2>&1 | tee "$proof_dir/native-proof.log"

mkdir "$output_dir"
docker create --name native-bootstrap-export \
    --label "io.authzlab.native-bootstrap-run=$bootstrap_run_id" \
    --entrypoint /bin/true "$bootstrap_image"
docker cp native-bootstrap-export:/usr/local/bin/lab-tool "$output_dir/bootstrap"
docker rm native-bootstrap-export
chmod 0555 "$output_dir/bootstrap"
python3 - "$core_dir" "$proof_dir" "$output_dir" "$repo_root" <<'PY'
import hashlib, json, os, sys
from pathlib import Path
source, evidence, output, repo = map(Path, sys.argv[1:])
proof = json.loads((evidence / "native-result.json").read_text())
image = (evidence / "image-id.txt").read_text().strip()
fixed_source = "53ff85cfdd3d84c26ae432189790e1e0f72d61db"
fixture = "d7f7ead003c774e30584798adfde38afb854528d"
def require(ok, message):
    if not ok:
        raise SystemExit(message)
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
require(proof["passed"] is True and proof["image_id"] == image and
        proof["source_commit"] == fixed_source and proof["packaging_commit"] == fixture and
        proof["browser_acceptance_verified"] is False, "Native proof identity differs")
require(len(proof["native_passes"]) == 2 and all(
        item.get("accepted") is True and item.get("statuses") == [403, 204, 200, 204, 403]
        for item in proof["native_passes"]), "Two genuine native sequences are required")
require(digest(output / "bootstrap") == proof["binary_sha256"],
        "Exported bootstrap differs from the tested image")
actual_fga = digest(repo / "artifacts/browser-openfga/openfga")
require(actual_fga == (evidence / "openfga.sha256").read_text().strip() ==
        proof["openfga_binary_sha256"], "OpenFGA identity changed during the proof")
require(digest(evidence / "native-proof.py") == digest(repo / "browser-single/bootstrap/native-proof.py"),
        "The versioned proof helper changed")
with (output / "native-tests.log").open("wb") as log:
    for name in ("wrapper-tests.log", "rustfmt.log", "unit-tests.log", "native-proof.log"):
        log.write(("\nLog: " + name + "\n").encode())
        log.write((evidence / name).read_bytes())
metadata = {
    "schema_version": 1, "source_commit": fixed_source, "target": "linux/amd64",
    "image_id": image, "binary_bytes": (output / "bootstrap").stat().st_size,
    "binary_sha256": digest(output / "bootstrap"),
    "dockerfile_sha256": digest(source / "browser-single/Dockerfile"),
    "cargo_lock_sha256": digest(source / "Cargo.lock"),
    "bootstrap_source_sha256": digest(source / "tools/bootstrap.rs"),
    "bootstrap_tests_sha256": digest(source / "tools/bootstrap_tests.rs"),
    "timeout_scope": "bootstrap_model_post_and_get",
    "default_model_timeout_seconds": 5, "min_model_timeout_seconds": 5,
    "max_model_timeout_seconds": 30, "browser_model_timeout_seconds": 30,
    "native_tests": "passed", "native_test_log_sha256": digest(output / "native-tests.log"),
    "packaging_commit": fixture, "native_proof_sha256": digest(evidence / "native-result.json"),
    "proof_origin_commit": "a0f9f9d08eca434ec2b03f756cf4bf5c08a0cb3b",
    "expected_openfga_sha_input": "required_verified_build_metadata",
    "native_harness_sha256": digest(evidence / "native-proof.py"),
    "producer_sha256": digest(repo / "browser-single/bootstrap/build.sh"),
    "openfga_binary_sha256": actual_fga, "openfga_build_sha256": digest(evidence / "openfga-build.json"),
    "workflow_commit": os.environ["GITHUB_SHA"], "run_id": os.environ["GITHUB_RUN_ID"],
    "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"], "browser_acceptance_verified": False}
(output / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
(output / "SOURCE.txt").write_text(
    "Bootstrap-only native artifact; not browser acceptance proof.\n"
    "Rust source commit: " + fixed_source + "\n"
    "Reviewed entrypoint fixture: " + fixture + "\n"
    "Tested native image: " + image + "\n"
    "Versioned producer/proof hashes are recorded in build.json.\n"
    "The retained rootfs probe and acceptance executables are not replaced.\n")
print(json.dumps(metadata, sort_keys=True), flush=True)
PY
