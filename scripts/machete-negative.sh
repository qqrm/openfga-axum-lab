#!/usr/bin/env bash
set -euo pipefail
tmp_dir="$(mktemp -d)"
trap 'rm -rf -- "$tmp_dir"' EXIT
mkdir -p "$tmp_dir/src"
cat > "$tmp_dir/Cargo.toml" <<'TOML'
[package]
name = "unused-dependency-fixture"
version = "0.0.0"
edition = "2024"
[dependencies]
serde = "1"
TOML
printf 'pub fn example() {}\n' > "$tmp_dir/src/lib.rs"
if cargo machete "$tmp_dir" >"$tmp_dir/report" 2>&1; then
  echo 'cargo-machete missed deliberately unused serde' >&2
  exit 1
fi
if ! grep -q 'serde' "$tmp_dir/report"; then
  cat "$tmp_dir/report" >&2
  echo 'cargo-machete failed for the wrong reason' >&2
  exit 1
fi
echo 'cargo-machete detected the deliberately unused dependency'
