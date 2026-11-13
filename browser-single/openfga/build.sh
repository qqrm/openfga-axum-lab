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
    "$patch_dir/browser_http_warmup.go" "$patch_dir/browser_http_warmup_test.go" \
    "$patch_dir/browser_model_codec.go" "$patch_dir/browser_model_codec_test.go" \
    "$patch_dir/browser_model_codec_integration_test.go" cmd/run/
cp "$patch_dir/browser_model_validation.go" "$patch_dir/browser_model_validation_test.go" pkg/typesystem/
cp "$patch_dir/browser_write_typesystem_test.go" pkg/server/commands/
test -z "$(gofmt -l cmd/run/browser_http_dial.go cmd/run/browser_http_dial_test.go \
    cmd/run/browser_http_warmup.go cmd/run/browser_http_warmup_test.go \
    cmd/run/browser_model_codec.go cmd/run/browser_model_codec_test.go \
    cmd/run/browser_model_codec_integration_test.go \
    pkg/typesystem/browser_model_validation.go pkg/typesystem/browser_model_validation_test.go \
    pkg/server/commands/write.go pkg/server/write.go pkg/server/commands/browser_write_typesystem_test.go)"

# Resolve only the pinned upstream dependency graph and verify its checksums.
go mod download
go mod verify
mkdir -p "$output_dir"
go test -mod=readonly -count=1 -timeout=120s -v ./cmd/run -run '^TestBrowserHTTP' \
    2>&1 | tee "$output_dir/dial-tests.log"
go test -mod=readonly -count=1 -timeout=120s -v ./pkg/typesystem -run '^TestBrowserModel' \
    2>&1 | tee "$output_dir/model-validation-tests.log"
go test -mod=readonly -count=1 -timeout=120s -v ./pkg/server/commands \
    2>&1 | tee "$output_dir/write-typesystem-tests.log"
go test -mod=readonly -count=1 -timeout=120s -v ./pkg/server \
    -run '^(TestWrite|TestCheckWriteAuthz|TestAuthorizationModelInvalidSchemaVersion)$' \
    2>&1 | tee -a "$output_dir/write-typesystem-tests.log"
git diff --exit-code -- go.mod go.sum pkg/server/config/config.go

build_epoch=$(git show -s --format=%ct HEAD)
build_date=$(date -u --date="@$build_epoch" +'%Y-%m-%dT%H:%M:%SZ')
go build -mod=readonly -trimpath -buildvcs=false \
    -ldflags "-s -w -X github.com/openfga/openfga/internal/build.Version=v1.10.3-browser-startup.7 -X github.com/openfga/openfga/internal/build.Commit=$source_commit -X github.com/openfga/openfga/internal/build.Date=$build_date -X github.com/openfga/openfga/internal/build.ProjectName=openfga" \
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
  "startup_model_warmup": "in_process_codec_validation_with_exact_schema_cache",
  "model_validation_cache": "conditionless_exact_protobuf_body_except_id",
  "model_json_decoder": "pinned_json_decode_private_clone",
  "warmup_generated_validation": "ValidateAll",
  "tuple_write_typesystem_reuse": "same_request_store_and_model",
  "warmup_model_sha256": "$model_sha256",
  "warmup_model_bytes": 52709,
  "warmup_timeout_seconds": 60,
  "warmup_cycles": 1,
  "authorization_request_timeout_default_seconds": 3,
  "patch_sha256": "$(sha256_of "$patch_dir/startup-dial.patch")",
  "helper_sha256": "$(sha256_of "$patch_dir/browser_http_dial.go")",
  "tests_sha256": "$(sha256_of "$patch_dir/browser_http_dial_test.go")",
  "warmup_helper_sha256": "$(sha256_of "$patch_dir/browser_http_warmup.go")",
  "warmup_tests_sha256": "$(sha256_of "$patch_dir/browser_http_warmup_test.go")",
  "model_codec_helper_sha256": "$(sha256_of "$patch_dir/browser_model_codec.go")",
  "model_codec_tests_sha256": "$(sha256_of "$patch_dir/browser_model_codec_test.go")",
  "model_codec_integration_tests_sha256": "$(sha256_of "$patch_dir/browser_model_codec_integration_test.go")",
  "model_validation_helper_sha256": "$(sha256_of "$patch_dir/browser_model_validation.go")",
  "model_validation_tests_sha256": "$(sha256_of "$patch_dir/browser_model_validation_test.go")",
  "write_typesystem_tests_sha256": "$(sha256_of "$patch_dir/browser_write_typesystem_test.go")",
  "go_sum_sha256": "$(sha256_of go.sum)",
  "binary_sha256": "$(sha256_of "$output_dir/openfga")",
  "startup_dial_tests": "passed",
  "transport_diagnostics_tests": "passed",
  "warmup_tests": "passed",
  "model_codec_tests": "passed",
  "model_validation_tests": "passed",
  "write_typesystem_tests": "passed"
}
EOF
cp LICENSE "$output_dir/LICENSE"
cp "$patch_dir/startup-dial.patch" "$patch_dir/browser_http_dial.go" \
    "$patch_dir/browser_http_warmup.go" "$patch_dir/browser_model_validation.go" \
    "$patch_dir/browser_model_codec.go" "$patch_dir/browser_model_codec_test.go" \
    "$patch_dir/browser_model_codec_integration_test.go" \
    "$patch_dir/browser_write_typesystem_test.go" "$output_dir/"
cat >"$output_dir/SOURCE.txt" <<EOF
OpenFGA v1.10.3, Apache-2.0, source commit $source_commit.
Source: https://github.com/openfga/openfga/tree/$source_commit
Browser build v1.10.3-browser-startup.7 extends the HTTP gateway startup gRPC
self-dial budget from 3 to 60 seconds and observes parent cancellation.
It also logs HTTP and unary RPC start/end metadata to stderr without bodies,
query strings or headers, preserving handler inputs, outputs and errors.
Unary raw request/response logging and that interceptor's internal-error details
are omitted; metadata status logs and streaming logging remain. Other upstream
error logs, validation, authentication, timeout and recovery middleware remain.
Before datastore construction/listeners, one separate 60-second startup phase
reads FGA_MODEL once and verifies its exact 52709 bytes and SHA-256 $model_sha256.
Within that same budget, one cycle exercises the real JSONPb/protobuf codecs,
generated ValidateAll request validation and the full original TypeSystem validator without
any datastore, RPC or Check operation. After every stage succeeds, only the
validity of the exact conditionless model body is retained in a private clone.
Matching compares every protobuf field and unknown field except the generated ID.
The write-model command reuses this proven validity and still writes PostgreSQL.
Real resolver loads construct a fresh TypeSystem with the real ID; no TypeSystem,
graph, tuple, datastore existence or Check decision is shared by this cache.
All mismatching models use the original full validator. Failure or cancellation
prevents cache publication and readiness. Stage timings remain diagnostic only.
The same successful warmup prepares a private JSON-to-protobuf decoder entry.
Only an exact pinned raw JSON model body reuses an independent protobuf copy.
The original streaming JSON parser remains; every mismatching body and other
request type keeps the original JSONPb decoder. The real URL supplies store_id;
gRPC, validation, authentication, context and PostgreSQL writes remain mandatory.
This does not cache a response or a Check decision. Decoder parity and real
HTTP/gRPC/memory-datastore integration tests run in the native startup test gate.
After resolving the actual store/model and checking write authorization, the
server passes that same TypeSystem to its WriteCommand. Only an exact nonempty
store/model binding can reuse it; nil or mismatching bindings keep the original
model read and construction. Supported schema, tuple/condition validation,
duplicate/size/deletion checks, context and actual datastore.Write are preserved.
This removes the second construction within one Write request; it adds no
global TypeSystem cache and does not cache any tuple or authorization result.
Full command tests and server Write/authorization/schema tests passed; their
combined log and new test source are included in this output.
The startup caller is bounded and observes parent cancellation; a synchronous
worker may finish later, but cannot publish readiness and the CLI exits on error.
The blocking connection, TLS, request timeouts and authorization engine remain.
Rebuild with browser-single/OpenFGA.Dockerfile in qqrm/openfga-axum-lab.
The exact patch and injected helpers are included; digests are in build.json.
EOF
