#!/usr/bin/env bash
# Reuse only the kernel built from this exact recipe; otherwise rebuild it.
set -euo pipefail
cd "$(dirname "$0")/.."
recipe=browser-single/Kernel.Dockerfile
expected_recipe=3335b9ee06c3b299b6fbc701b943fc0f494a8fb2ac1b5573c0cec239fbe7783a
archive_sha=17a12602844ca7671ada3f0201e1e91b3d43a7b0020e9f361663a9a4a628b4ae
output=artifacts/browser-kernel
temporary=$(mktemp -d)
trap 'rm -rf -- "$temporary"' EXIT
actual_recipe=$(sha256sum "$recipe" | cut -d' ' -f1)
if [[ "$actual_recipe" == "$expected_recipe" ]] && \
    gh api repos/qqrm/openfga-axum-lab/actions/artifacts/13887514892/zip >"$temporary/kernel.zip"; then
    printf '%s  %s\n' "$archive_sha" "$temporary/kernel.zip" | sha256sum --check --status
    mkdir -p "$output"
    unzip -q "$temporary/kernel.zip" -d "$output"
    echo 'Reused the checksummed kernel built from the identical recipe.'
else
    docker buildx build --file "$recipe" --output "type=local,dest=$output" .
fi
grep -qx 'CONFIG_SYSVIPC=y' "$output/linux.config"
grep -qx 'CONFIG_IPC_NS=y' "$output/linux.config"
grep -qx 'CONFIG_HW_RANDOM=y' "$output/linux.config"
grep -qx 'CONFIG_HW_RANDOM_VIRTIO=y' "$output/linux.config"
grep -qx 'CONFIG_VIRTIO_PCI=y' "$output/linux.config"
test -s "$output/bzImage"
