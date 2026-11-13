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
        docker rm -f -v "$container" || true
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
core_commit=207d15fab685d3adb9c5d51a048982c2236ae75c
fixture_commit=4b7a1e1933083232fb8305a60a47bdd3c32e6689
test "$(git -C "$core_dir" rev-parse HEAD)" = "$core_commit"
test "$(git -C "$fixture_dir" rev-parse HEAD)" = "$fixture_commit"
git -C "$core_dir" diff --exit-code HEAD --
git -C "$fixture_dir" diff --exit-code HEAD --
test "$(git -C "$core_dir" hash-object tools/bootstrap.rs)" = a5e2723444690dfc61461eb744fa6f5c37891a84
test "$(git -C "$core_dir" hash-object tools/bootstrap_tests.rs)" = 9593ee9c2236c7c28cd6ec366201f6f33a0a683d
# Only the separately built bootstrap overlay opts into the new deny probe.
# Keep the main CLI source at its reviewed pre-probe bytes.
test "$(git -C "$repo_root" hash-object tools/bootstrap.rs)" = a38ba1e8f792e7c0ed019c92f0ffd08a5d74a7a3
test "$(git -C "$repo_root" hash-object tools/bootstrap_tests.rs)" = f5b5e56b04f9d8dd518228f84b7470b9949c65ca
for relative in tools/bootstrap.rs tools/bootstrap_tests.rs; do
    cmp "$core_dir/$relative" "$fixture_dir/$relative"
done
for relative in Cargo.toml Cargo.lock browser-single/Dockerfile fga/model.json; do
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
cp "$repo_root/browser-single/bootstrap/native-proof.mjs" "$proof_dir/native-proof.mjs"
chmod 0644 "$proof_dir/native-proof.mjs"
export GITHUB_RUN_ID="$bootstrap_run_id"
export GITHUB_RUN_ATTEMPT=${GITHUB_RUN_ATTEMPT:-1}
if [[ -z "${GITHUB_SHA:-}" ]]; then
    GITHUB_SHA=$(git -C "$repo_root" rev-parse HEAD)
fi
export GITHUB_SHA

# Consume the already built and regression-tested OpenFGA artifact. No download,
# fallback executable, OpenFGA build or emulator build occurs in this producer.
node --input-type=module - "$repo_root" "$proof_dir" <<'NODE'
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFileSync, statSync, writeFileSync} from 'node:fs';
import path from 'node:path';
const [repo, evidence] = process.argv.slice(2);
const directory = path.join(repo, 'artifacts/browser-openfga');
const digest = filename => createHash('sha256').update(readFileSync(filename)).digest('hex');
const build = JSON.parse(readFileSync(path.join(directory, 'build.json'), 'utf8'));
const binary = path.join(directory, 'openfga');
const info = statSync(binary);
assert(info.isFile() && (info.mode & 0o111), 'OpenFGA executable is unavailable');
const actual = digest(binary);
assert.match(build.binary_sha256, /^[0-9a-f]{64}$/, 'Invalid OpenFGA metadata hash');
assert.equal(actual, build.binary_sha256, 'OpenFGA metadata/binary identity differs');
for (const [key, value] of Object.entries({
    source_commit: '293893b22a53d8a130a5f7c97cab4bc8e91bcaa4',
    upstream_version: '1.10.3', startup_dial_timeout_seconds: 60,
    authorization_request_timeout_default_seconds: 3,
    warmup_timeout_seconds: 60, warmup_cycles: 1,
    startup_dial_tests: 'passed', warmup_tests: 'passed',
    model_codec_tests: 'passed', model_validation_tests: 'passed',
    model_json_encoder: 'single_exact_real_model_response',
    first_resolver_typesystem_reuse: 'one_shot_after_real_model_read',
    model_reuse_race_tests: 'passed',
    write_typesystem_tests: 'passed',
    warmup_model_sha256: 'a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d'
})) assert.equal(build[key], value, 'Unexpected verified OpenFGA field: ' + key);
for (const [key, name] of Object.entries({
    patch_sha256: 'startup-dial.patch',
    helper_sha256: 'browser_http_dial.go',
    tests_sha256: 'browser_http_dial_test.go',
    warmup_helper_sha256: 'browser_http_warmup.go',
    warmup_tests_sha256: 'browser_http_warmup_test.go',
    model_codec_helper_sha256: 'browser_model_codec.go',
    model_codec_tests_sha256: 'browser_model_codec_test.go',
    model_codec_integration_tests_sha256: 'browser_model_codec_integration_test.go',
    model_validation_helper_sha256: 'browser_model_validation.go',
    model_validation_tests_sha256: 'browser_model_validation_test.go',
    write_typesystem_tests_sha256: 'browser_write_typesystem_test.go',
    resolver_transfer_integration_tests_sha256: 'browser_resolver_transfer_integration_test.go'
})) {
    assert.equal(build[key], digest(path.join(repo, 'browser-single/openfga', name)),
        'OpenFGA source/proof identity differs: ' + key);
}
assert.equal(digest(path.join(repo, 'fga/model.json')), build.warmup_model_sha256, 'Pinned model changed');
assert.equal(build.model_reuse_race_tests_sha256,
    digest(path.join(directory, 'model-reuse-race-tests.log')), 'OpenFGA race-test log identity differs');
writeFileSync(path.join(evidence, 'openfga.sha256'), actual + '\n');
writeFileSync(path.join(evidence, 'openfga-build.json'), JSON.stringify(build, null, 2) + '\n');
console.log(JSON.stringify({scope: 'reuse verified OpenFGA build', binary_sha256: actual,
    binary_bytes: info.size, source_commit: build.source_commit}));
NODE
IFS= read -r openfga_sha <"$proof_dir/openfga.sha256"

# A local Buildx export may retain directory permissions inaccessible to the
# unprivileged service user. Stage only the verified executable in an owned,
# traversable directory; keep the producer's OpenFGA output untouched.
mkdir -m 0755 "$bootstrap_workdir/artifacts/native-bootstrap-input"
install -m 0555 "$repo_root/artifacts/browser-openfga/openfga" \
    "$bootstrap_workdir/artifacts/native-bootstrap-input/openfga"
printf '%s  %s\n' "$openfga_sha" "$bootstrap_workdir/artifacts/native-bootstrap-input/openfga" \
    | sha256sum --check --status
node --check "$proof_dir/native-proof.mjs"
bash -euo pipefail -s -- "$repo_root" <<'CHECK' 2>&1 | tee "$proof_dir/wrapper-tests.log"
repo_root=$1
node --check "$repo_root/browser-single/pages/startup-payload.mjs"
node --check "$repo_root/browser-single/startup-payload.test.mjs"
node --check "$repo_root/browser-single/check-pages.mjs"
node --check "$repo_root/browser-single/bootstrap/native-proof.mjs"
node --test "$repo_root/browser-single/startup-payload.test.mjs"
for script in "$repo_root/browser-single/entrypoint.sh" \
              "$repo_root/browser-single/pages/guest.sh" \
              "$repo_root/browser-single/prepare-pages.sh" \
              "$repo_root/browser-single/bootstrap/build.sh"; do
    bash -n "$script"
    shellcheck "$script"
done
CHECK

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
grep -Fq 'test result: ok. 25 passed; 0 failed;' "$proof_dir/unit-tests.log"

cd "$bootstrap_workdir"
BROWSER_BOOTSTRAP_OPENFGA_SHA256="$openfga_sha" \
    timeout --kill-after=15s 720s node artifacts/native-bootstrap/native-proof.mjs \
    2>&1 | tee "$proof_dir/native-proof.log"
cd "$repo_root"

mkdir "$output_dir"
docker create --name native-bootstrap-export \
    --label "io.authzlab.native-bootstrap-run=$bootstrap_run_id" \
    --entrypoint /bin/true "$bootstrap_image"
docker cp native-bootstrap-export:/usr/local/bin/lab-tool "$output_dir/bootstrap"
test "$(docker inspect -f '{{index .Config.Labels "io.authzlab.native-bootstrap-run"}}' native-bootstrap-export)" = "$bootstrap_run_id"
docker rm -v native-bootstrap-export
chmod 0555 "$output_dir/bootstrap"
node --input-type=module - "$core_dir" "$proof_dir" "$output_dir" "$repo_root" <<'NODE'
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {closeSync, openSync, readFileSync, statSync, writeFileSync} from 'node:fs';
import path from 'node:path';
const [source, evidence, output, repo] = process.argv.slice(2);
const read = (directory, name) => readFileSync(path.join(directory, name), 'utf8');
const digest = filename => createHash('sha256').update(readFileSync(filename)).digest('hex');
const proof = JSON.parse(read(evidence, 'native-result.json'));
const image = read(evidence, 'image-id.txt').trim();
const fixedSource = '207d15fab685d3adb9c5d51a048982c2236ae75c';
const fixture = '4b7a1e1933083232fb8305a60a47bdd3c32e6689';
assert(proof.passed === true && proof.image_id === image &&
    proof.source_commit === fixedSource && proof.packaging_commit === fixture &&
    proof.browser_acceptance_verified === false, 'Native proof identity differs');
assert.deepEqual(proof.default_failures.map(item => item.phase), ['default-post', 'default-get']);
for (const item of [...proof.default_failures, proof.empty_store_check_deadline_failure]) {
    assert.equal(item.exit, 1, 'A delayed ordinary request must fail');
    assert.equal(item.config_published, false, 'A failed request must not publish a binding');
    assert.equal(item.response_still_withheld, true, 'The response must still be delayed at failure');
    assert(item.elapsed_ms >= 4500 && item.elapsed_ms < 6000, 'The original five-second limit changed');
}
assert.equal(proof.empty_store_check_deadline_failure.phase, 'check-deadline');
assert.equal(proof.delayed_empty_store_check_seconds, 6);
assert.equal(proof.actual_postgres_model_count, 4);
assert.equal(proof.actual_postgres_tuple_count_before_app, 0);
for (const check of [proof.empty_store_check, proof.empty_store_check_deadline_failure.real_check]) {
    assert.equal(check.request_count, 1, 'Exactly one real Check is required');
    assert.equal(check.status, 200);
    assert.equal(check.allowed, false, 'The real new store must deny ownership');
    assert.equal(check.consistency, 'HIGHER_CONSISTENCY');
    assert.equal(check.tuple_count_before, 0);
    assert.deepEqual(check.tuple_key, {
        user: 'user:browser-bootstrap-probe', relation: 'ownership',
        object: 'lakekeeper_table:browser-bootstrap-probe'
    });
    assert.match(check.store_id, /^[0-9A-HJKMNP-TV-Z]{26}$/);
    assert.match(check.authorization_model_id, /^[0-9A-HJKMNP-TV-Z]{26}$/);
}
assert.equal(proof.native_passes.length, 2, 'Two genuine native sequences are required');
for (const item of proof.native_passes) {
    assert.equal(item.accepted, true, 'Original native acceptance failed');
    assert.deepEqual(item.statuses, [403, 204, 200, 204, 403], 'Native sequence differs');
}
assert.equal(digest(path.join(output, 'bootstrap')), proof.binary_sha256,
    'Exported bootstrap differs from the tested image');
const actualFga = digest(path.join(repo, 'artifacts/browser-openfga/openfga'));
assert.equal(actualFga, read(evidence, 'openfga.sha256').trim(), 'OpenFGA input changed');
assert.equal(actualFga, proof.openfga_binary_sha256, 'OpenFGA identity changed during the proof');
assert.equal(digest(path.join(evidence, 'native-proof.mjs')),
    digest(path.join(repo, 'browser-single/bootstrap/native-proof.mjs')), 'The versioned proof helper changed');
const log = openSync(path.join(output, 'native-tests.log'), 'w');
try {
    for (const name of ['wrapper-tests.log', 'rustfmt.log', 'unit-tests.log', 'native-proof.log']) {
        writeFileSync(log, '\nLog: ' + name + '\n');
        writeFileSync(log, readFileSync(path.join(evidence, name)));
    }
} finally {
    closeSync(log);
}
const metadata = {
    schema_version: 1, source_commit: fixedSource, target: 'linux/amd64',
    image_id: image, binary_bytes: statSync(path.join(output, 'bootstrap')).size,
    binary_sha256: digest(path.join(output, 'bootstrap')),
    dockerfile_sha256: digest(path.join(source, 'browser-single/Dockerfile')),
    cargo_toml_sha256: digest(path.join(source, 'Cargo.toml')),
    cargo_lock_sha256: digest(path.join(source, 'Cargo.lock')),
    bootstrap_source_sha256: digest(path.join(source, 'tools/bootstrap.rs')),
    bootstrap_tests_sha256: digest(path.join(source, 'tools/bootstrap_tests.rs')),
    timeout_scope: 'bootstrap_model_post_and_get',
    default_model_timeout_seconds: 5, min_model_timeout_seconds: 5,
    max_model_timeout_seconds: 30, browser_model_timeout_seconds: 30,
    empty_store_check: 'one_real_higher_consistency_ownership_deny',
    empty_store_check_default: false, browser_empty_store_check: true,
    empty_store_check_timeout_seconds: 5, empty_store_check_tests: 'passed',
    native_tests: 'passed', native_test_log_sha256: digest(path.join(output, 'native-tests.log')),
    packaging_commit: fixture, native_proof_sha256: digest(path.join(evidence, 'native-result.json')),
    proof_semantics_origin_commit: 'c8cf8630d86712567935e064dabd8b437640c8c6',
    native_harness_runtime: 'node',
    expected_openfga_sha_input: 'required_verified_build_metadata',
    native_harness_sha256: digest(path.join(evidence, 'native-proof.mjs')),
    producer_sha256: digest(path.join(repo, 'browser-single/bootstrap/build.sh')),
    openfga_binary_sha256: actualFga, openfga_build_sha256: digest(path.join(evidence, 'openfga-build.json')),
    workflow_commit: process.env.GITHUB_SHA, run_id: process.env.GITHUB_RUN_ID,
    run_attempt: process.env.GITHUB_RUN_ATTEMPT, browser_acceptance_verified: false
};
writeFileSync(path.join(output, 'build.json'), JSON.stringify(metadata, null, 2) + '\n');
writeFileSync(path.join(output, 'SOURCE.txt'),
    'Bootstrap-only native artifact; not browser acceptance proof.\n' +
    'Rust source commit: ' + fixedSource + '\n' +
    'Reviewed entrypoint fixture: ' + fixture + '\n' +
    'Tested native image: ' + image + '\n' +
    'Versioned Node producer/proof hashes are recorded in build.json.\n' +
    'The retained rootfs probe and acceptance executables are not replaced.\n' +
    'The opt-in empty-store Check requires a real deny with the original five-second client limit.\n');
console.log(JSON.stringify(metadata));
NODE
