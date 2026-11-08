#!/usr/bin/env bash
set -euo pipefail
output_dir="${1:-browser-output}"
mkdir -p "$output_dir"
rustc --edition 2024 --target wasm32-unknown-unknown --crate-type cdylib \
  -C opt-level=z -C panic=abort -C strip=symbols \
  browser-demo/domain_entry.rs -o "$output_dir/domain.wasm"
cp browser-demo/index.html browser-demo/style.css browser-demo/app.js "$output_dir/"
cp fga/model.json fga/provenance.json "$output_dir/"
cp NOTICE "$output_dir/"
mkdir -p "$output_dir/LICENSES"
cp LICENSES/Apache-2.0-Openlane.txt "$output_dir/LICENSES/"
printf 'Browser demo built in %s\n' "$output_dir"
