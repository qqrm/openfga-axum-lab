#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
output_dir="${1:-browser-output}"
mkdir -p "$output_dir"
cargo build --locked --offline \
  --manifest-path "$repo_root/browser-demo/Cargo.toml" \
  --target wasm32-unknown-unknown --profile wasm \
  --target-dir "$repo_root/target/browser-demo"
cp "$repo_root/target/browser-demo/wasm32-unknown-unknown/wasm/openfga_axum_browser_demo.wasm" "$output_dir/domain.wasm"
cp "$repo_root/browser-demo/index.html" "$repo_root/browser-demo/style.css" "$repo_root/browser-demo/app.js" "$output_dir/"
printf 'Browser demo built in %s\n' "$output_dir"
