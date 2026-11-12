#!/usr/bin/env bash
# Run inside a clean checkout of the exact upstream commit, with Go 1.24.6.
set -Eeuo pipefail

patch_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
output_dir=${1:-/out}
source_commit=293893b22a53d8a130a5f7c97cab4bc8e91bcaa4
go_version=go1.24.6
export GOTOOLCHAIN=local CGO_ENABLED=0 GOOS=linux GOARCH=amd64

test "$(git rev-parse HEAD)" = "$source_commit"
test "$(go env GOVERSION)" = "$go_version"
git diff --exit-code --quiet HEAD
sha256sum --check "$patch_dir/upstream.sha256"
git apply --unidiff-zero --check --whitespace=error-all "$patch_dir/startup-dial.patch"
git apply --unidiff-zero --whitespace=error-all "$patch_dir/startup-dial.patch"
cp "$patch_dir/browser_http_dial.go" "$patch_dir/browser_http_dial_test.go" cmd/run/
test -z "$(gofmt -l cmd/run/browser_http_dial.go cmd/run/browser_http_dial_test.go)"

# Resolve only the pinned upstream dependency graph and verify its checksums.
go mod download
go mod verify
mkdir -p "$output_dir"
go test -mod=readonly -count=1 -timeout=120s -v ./cmd/run -run '^TestBrowserHTTP' \
    2>&1 | tee "$output_dir/dial-tests.log"
git diff --exit-code -- go.mod go.sum pkg/server/config/config.go

build_epoch=$(git show -s --format=%ct HEAD)
build_date=$(date -u --date="@$build_epoch" +'%Y-%m-%dT%H:%M:%SZ')
go build -mod=readonly -trimpath -buildvcs=false \
    -ldflags "-s -w -X github.com/openfga/openfga/internal/build.Version=v1.10.3-browser-startup.1 -X github.com/openfga/openfga/internal/build.Commit=$source_commit -X github.com/openfga/openfga/internal/build.Date=$build_date -X github.com/openfga/openfga/internal/build.ProjectName=openfga" \
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
  "authorization_request_timeout_default_seconds": 3,
  "patch_sha256": "$(sha256_of "$patch_dir/startup-dial.patch")",
  "helper_sha256": "$(sha256_of "$patch_dir/browser_http_dial.go")",
  "tests_sha256": "$(sha256_of "$patch_dir/browser_http_dial_test.go")",
  "go_sum_sha256": "$(sha256_of go.sum)",
  "binary_sha256": "$(sha256_of "$output_dir/openfga")",
  "startup_dial_tests": "passed",
  "transport_diagnostics_tests": "passed"
}
EOF
cp LICENSE "$output_dir/LICENSE"
cp "$patch_dir/startup-dial.patch" "$patch_dir/browser_http_dial.go" "$output_dir/"
cat >"$output_dir/SOURCE.txt" <<EOF
OpenFGA v1.10.3, Apache-2.0, source commit $source_commit.
Source: https://github.com/openfga/openfga/tree/$source_commit
Browser build v1.10.3-browser-startup.1 extends the HTTP gateway startup gRPC
self-dial budget from 3 to 60 seconds and observes parent cancellation.
It also logs HTTP and unary RPC start/end metadata to stderr without bodies,
query strings or headers, preserving handler inputs, outputs and errors.
The blocking connection, TLS, request timeouts and authorization engine remain.
Rebuild with browser-single/OpenFGA.Dockerfile in qqrm/openfga-axum-lab.
The exact patch and injected helper are included; digests are in build.json.
EOF
