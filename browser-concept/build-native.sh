#!/usr/bin/env bash
# Build only the new concept executable; retained VM/runtime inputs are separate.
set -euo pipefail
umask 022
repo_root=$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
cd "$repo_root"
: "${GITHUB_SHA:?the exact producer commit is required}"
: "${GITHUB_RUN_ID:?the producer run is required}"
: "${GITHUB_RUN_ATTEMPT:?the producer attempt is required}"
[[ "$GITHUB_RUN_ATTEMPT" == 1 ]]
[[ "$(git rev-parse HEAD)" == "$GITHUB_SHA" ]]
git diff --exit-code HEAD --
native_output="$repo_root/artifacts/browser-concept-native"
test ! -e "$native_output"
mkdir -p "$native_output"

node browser-concept/build-provenance.mjs prepare "$native_output"

# Resolve the reviewed compiler tag once, then build from its exact pulled digest.
builder_tag=rust:1.99.1-slim-bookworm
docker pull --platform linux/amd64 "$builder_tag"
docker image inspect "$builder_tag" > "$native_output/builder-image.json"
builder_digest=$(docker image inspect "$builder_tag" --format '{{index .RepoDigests 0}}')
[[ "$builder_digest" =~ ^rust@sha256:[0-9a-f]{64}$ ]]
docker buildx build --platform linux/amd64 --progress plain \
    --file browser-concept/Dockerfile --target output \
    --build-arg "RUST_IMAGE=$builder_digest" \
    --output "type=local,dest=$native_output" . \
    2>&1 | tee "$native_output/build.log"

node browser-concept/build-provenance.mjs finish "$native_output" "$builder_tag" "$builder_digest"
