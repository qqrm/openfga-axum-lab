#!/usr/bin/env bash
set -euo pipefail
tmp_dir="$(mktemp -d)"
trap 'rm -rf -- "$tmp_dir"' EXIT
cat > "$tmp_dir/Cargo.lock" <<'LOCK'
version = 3
[[package]]
name = "rsa"
version = "0.9.10"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000000000000000000000000000000000000000000000000000000000000000"
LOCK
if cargo audit --no-fetch --no-yanked --file "$tmp_dir/Cargo.lock" >"$tmp_dir/report" 2>&1; then
  echo 'cargo-audit negative control unexpectedly passed' >&2
  exit 1
fi
if ! grep -q 'RUSTSEC-2023-0071' "$tmp_dir/report"; then
  cat "$tmp_dir/report" >&2
  echo 'cargo-audit failed for the wrong reason' >&2
  exit 1
fi
echo 'cargo-audit detected the known vulnerable fixture'
