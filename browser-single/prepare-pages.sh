#!/usr/bin/env bash
# Materialize a pinned, reviewed runtime; never silently publish a different build.
set -euo pipefail
cd "$(dirname "$0")/.."
manifest=browser-single/pages/runtime-manifest.json
output=browser-output/wasm
mkdir -p "$output/runtime"
cp -a browser-single/pages/. "$output/"
temporary=$(mktemp -d)
trap 'rm -rf -- "$temporary"' EXIT
artifact=$(jq -er '.artifact_id' "$manifest")
archive_sha=$(jq -er '.archive_sha256' "$manifest")
if gh api "repos/qqrm/openfga-axum-lab/actions/artifacts/$artifact/zip" >"$temporary/runtime.zip"; then
    printf '%s  %s\n' "$archive_sha" "$temporary/runtime.zip" | sha256sum --check --status
    # Hash validation authenticates the exact ZIP, including its entry paths.
    unzip -q "$temporary/runtime.zip" -d "$output/runtime"
else
    # Actions artifacts expire. Existing Pages keeps these exact immutable bytes;
    # verify every file rather than relying on an expiring artifact indefinitely.
    echo 'Actions archive unavailable; retrieving the identical published snapshot.' >&2
    while IFS= read -r name; do
        [[ "$name" =~ ^[A-Za-z0-9._-]+$ ]] || exit 1
        curl --fail --silent --show-error --location --retry 2 --max-time 300 \
            "https://qqrm.github.io/openfga-axum-lab/wasm/runtime/$name" \
            --output "$output/runtime/$name"
    done < <(jq -r '.files | keys[]' "$manifest")
fi
jq -r '.files | to_entries[] | "\(.value.sha256)  \(.key)"' "$manifest" >"$temporary/files.sha256"
(cd "$output/runtime" && sha256sum --check "$temporary/files.sha256")
node --check "$output/launcher.js"
node --check "$output/isolation-sw.js"
bash -n "$output/guest.sh"
node --input-type=module - <<'JS'
import {readFileSync,writeFileSync} from 'node:fs';
const path='browser-output/index.html';
const html=readFileSync(path,'utf8');
if(!html.includes('</main>'))throw new Error('Missing model explorer main element');
writeFileSync(path,html.replace('</main>','<section class="panel"><h2>Experimental browser Linux lab</h2><p>Run the real PostgreSQL, OpenFGA and Axum binaries locally in your tab. Large download; explicit opt-in; synthetic data only.</p><a href="wasm/">Open the browser experiment</a></section></main>'));
JS
printf 'Pinned browser runtime prepared; source: %s\n' "$(jq -r '.source_commit' "$manifest")"
