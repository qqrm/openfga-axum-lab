#!/usr/bin/env bash
set -euo pipefail
tmp_dir="$(mktemp -d)"
trap 'rm -rf -- "$tmp_dir"' EXIT
# Both advisories ignored for the main lockfile must still be detectable.
# This isolated check deliberately does not inherit their exceptions.
for spec in 'rsa 0.9.10 RUSTSEC-2023-0071' 'libsqlite3-sys 0.37.0 RUSTSEC-2026-0459'; do
  read -r package version advisory <<< "$spec"
  cat > "$tmp_dir/Cargo.lock" <<LOCK
version = 3
[[package]]
name = "$package"
version = "$version"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000000000000000000000000000000000000000000000000000000000000000"
LOCK
  if cargo audit --no-fetch --no-yanked --file "$tmp_dir/Cargo.lock" >"$tmp_dir/report" 2>&1; then
    echo "cargo-audit negative control for $package unexpectedly passed" >&2
    exit 1
  fi
  if ! grep -q "$advisory" "$tmp_dir/report"; then
    cat "$tmp_dir/report" >&2
    echo "cargo-audit failed for the wrong reason: $advisory" >&2
    exit 1
  fi
done
echo 'cargo-audit detected both lockfile advisory fixtures'
