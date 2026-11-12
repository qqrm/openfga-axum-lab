#!/usr/bin/env bash
# Run inside a clean checkout of the exact upstream commit, with Go 1.24.6.
set -Eeuo pipefail

patch_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
output_dir=${1:-/out}
model_fixture=${2:-"$patch_dir/../../fga/model.json"}
model_sha256=a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d
source_commit=293893b22a53d8a130a5f7c97cab4bc8e91bcaa4
go_version=go1.24.6
export GOTOOLCHAIN=local CGO_ENABLED=0 GOOS=linux GOARCH=amd64

model_fixture=$(realpath -- "$model_fixture")
printf '%s  %s\n' "$model_sha256" "$model_fixture" | sha256sum --check -
export BROWSER_OPENFGA_TEST_MODEL="$model_fixture"
test "$(git rev-parse HEAD)" = "$source_commit"
test "$(go env GOVERSION)" = "$go_version"
git diff --exit-code --quiet HEAD
sha256sum --check "$patch_dir/upstream.sha256"
git apply --unidiff-zero --check --whitespace=error-all "$patch_dir/startup-dial.patch"
git apply --unidiff-zero --whitespace=error-all "$patch_dir/startup-dial.patch"
cp "$patch_dir/browser_http_dial.go" "$patch_dir/browser_http_dial_test.go" \
    "$patch_dir/browser_http_warmup.go" "$patch_dir/browser_http_warmup_test.go" cmd/run/
test -z "$(gofmt -l cmd/run/browser_http_dial.go cmd/run/browser_http_dial_test.go \
    cmd/run/browser_http_warmup.go cmd/run/browser_http_warmup_test.go)"

# Resolve only the pinned upstream dependency graph and verify its checksums.
go mod download
go mod verify
mkdir -p "$output_dir"
go test -mod=readonly -count=1 -timeout=120s -v ./cmd/run -run '^TestBrowserHTTP' \
    2>&1 | tee "$output_dir/dial-tests.log"
git diff --exit-code -- go.mod go.sum pkg/server/config/config.go \
    pkg/server/commands/write_authzmodel.go pkg/typesystem/typesystem.go

build_epoch=$(git show -s --format=%ct HEAD)
build_date=$(date -u --date="@$build_epoch" +'%Y-%m-%dT%H:%M:%SZ')
go build -mod=readonly -trimpath -buildvcs=false \
    -ldflags "-s -w -X github.com/openfga/openfga/internal/build.Version=v1.10.3-browser-startup.2 -X github.com/openfga/openfga/internal/build.Commit=$source_commit -X github.com/openfga/openfga/internal/build.Date=$build_date -X github.com/openfga/openfga/internal/build.ProjectName=openfga" \
    -o "$output_dir/openfga" ./cmd/openfga
chmod 0555 "$output_dir/openfga"

sha256_of() {
    local digest
    digest=$(sha256sum "$1")
    printf '%s' "${digest%% *}"
}

cat >"$output_dir/build.json" <<EOF
{
  "upstream_version": "1.10.3",
  "source_commit": "$source_commit",
  "go_version": "$go_version",
  "build_date": "$build_date",
  "target": "linux/amd64",
  "startup_dial_timeout_seconds": 60,
  "transport_diagnostics": "http_and_unary_rpc_metadata_only",
  "unary_raw_logging": "disabled_browser_only",
  "startup_model_warmup": "in_process_codec_validation_only",
  "warmup_model_sha256": "$model_sha256",
  "warmup_model_bytes": 52709,
  "warmup_timeout_seconds": 60,
  "authorization_request_timeout_default_seconds": 3,
  "patch_sha256": "$(sha256_of "$patch_dir/startup-dial.patch")",
  "helper_sha256": "$(sha256_of "$patch_dir/browser_http_dial.go")",
  "tests_sha256": "$(sha256_of "$patch_dir/browser_http_dial_test.go")",
  "warmup_helper_sha256": "$(sha256_of "$patch_dir/browser_http_warmup.go")",
  "warmup_tests_sha256": "$(sha256_of "$patch_dir/browser_http_warmup_test.go")",
  "go_sum_sha256": "$(sha256_of go.sum)",
  "binary_sha256": "$(sha256_of "$output_dir/openfga")",
  "startup_dial_tests": "passed",
  "transport_diagnostics_tests": "passed",
  "warmup_tests": "passed"
}
EOF
cp LICENSE "$output_dir/LICENSE"
cp "$patch_dir/startup-dial.patch" "$patch_dir/browser_http_dial.go" \
    "$patch_dir/browser_http_warmup.go" "$output_dir/"
cat >"$output_dir/SOURCE.txt" <<EOF
OpenFGA v1.10.3, Apache-2.0, source commit $source_commit.
Source: https://github.com/openfga/openfga/tree/$source_commit
Browser build v1.10.3-browser-startup.2 extends the HTTP gateway startup gRPC
self-dial budget from 3 to 60 seconds and observes parent cancellation.
It also logs HTTP and unary RPC start/end metadata to stderr without bodies,
query strings or headers, preserving handler inputs, outputs and errors.
Unary raw request/response logging and that interceptor's internal-error details
are omitted; metadata status logs and streaming logging remain. Other upstream
error logs, validation, authentication, timeout and recovery middleware remain.
Before datastore construction/listeners, a separate 60-second startup phase
reads FGA_MODEL and verifies its exact 52709 bytes and SHA-256 $model_sha256.
It exercises the real JSONPb/protobuf codecs, generated request validation and
TypeSystem.NewAndValidate without any datastore, RPC or Check operation. Local
model/response fixtures and validation results are discarded; real operations
repeat the unchanged validation and datastore work. Failure prevents readiness.
The startup caller is bounded and observes parent cancellation; a synchronous
worker may finish later, but cannot publish readiness and the CLI exits on error.
The blocking connection, TLS, request timeouts and authorization engine remain.
Rebuild with browser-single/OpenFGA.Dockerfile in qqrm/openfga-axum-lab.
The exact patch and injected helpers are included; digests are in build.json.
EOF
