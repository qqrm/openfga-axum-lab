#!/usr/bin/env bash
# Actual OpenFGA model/adapter checks; no PostgreSQL or browser substitute.
set -euo pipefail
cd "$(dirname "$0")/.."
tools_dir=$(mktemp -d)
trap 'rm -rf -- "$tools_dir"' EXIT

if [[ -z "${OPENFGA_TEST_BINARY:-}" || -z "${FGA_CLI_TEST_BINARY:-}" ]]; then
    if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
        echo 'Set OPENFGA_TEST_BINARY and FGA_CLI_TEST_BINARY for this platform.' >&2
        exit 1
    fi
fi

if [[ -z "${OPENFGA_TEST_BINARY:-}" ]]; then
    archive="$tools_dir/openfga.tar.gz"
    curl --fail --silent --show-error --location --retry 2 --max-time 120 \
        https://github.com/openfga/openfga/releases/download/v1.10.3/openfga_1.10.3_linux_amd64.tar.gz \
        --output "$archive"
    printf '%s  %s\n' cbf0ddaddd9580dc0b9d6af70265853c53b4847ff1ac91a9fd06b97ed6f97efb "$archive" | sha256sum --check --status
    tar -xzf "$archive" -C "$tools_dir" openfga
    export OPENFGA_TEST_BINARY="$tools_dir/openfga"
fi
if [[ -z "${FGA_CLI_TEST_BINARY:-}" ]]; then
    archive="$tools_dir/fga.tar.gz"
    curl --fail --silent --show-error --location --retry 2 --max-time 120 \
        https://github.com/openfga/cli/releases/download/v0.8.2/fga_0.8.2_linux_amd64.tar.gz \
        --output "$archive"
    printf '%s  %s\n' e48c4a73cf68031fe9fc0827f1462a9b2d6981e3d2e9b82f1e1c2e120c293443 "$archive" | sha256sum --check --status
    tar -xzf "$archive" -C "$tools_dir" fga
    export FGA_CLI_TEST_BINARY="$tools_dir/fga"
fi
for transport in http grpc; do
    cargo test --locked --no-default-features --features "$transport" \
        --test fga_model --test "${transport}_adapter" -- --include-ignored --nocapture
done
