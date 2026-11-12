#!/usr/bin/env bash
# Materialize a pinned, reviewed runtime; never silently publish a different build.
set -euo pipefail
cd "$(dirname "$0")/.."
manifest=browser-single/pages/runtime-manifest.json
output=browser-output/wasm
openfga_build=artifacts/browser-openfga
test -s "$openfga_build/openfga"
test -s "$openfga_build/build.json"
mkdir -p "$output/runtime"
cp -a browser-single/pages/. "$output/"
cp browser-single/entrypoint.sh "$output/entrypoint.sh"
# Replace the generated executable, which the build deliberately marks read-only.
cp -f "$openfga_build/openfga" "$output/runtime/openfga"
mkdir -p "$output/runtime/openfga-build"
cp "$openfga_build/build.json" "$openfga_build/LICENSE" "$openfga_build/SOURCE.txt" \
    "$openfga_build/startup-dial.patch" "$openfga_build/browser_http_dial.go" \
    "$openfga_build/browser_http_warmup.go" \
    "$openfga_build/dial-tests.log" "$output/runtime/openfga-build/"
temporary=$(mktemp -d)
trap 'rm -rf -- "$temporary"' EXIT
artifact=$(jq -er '.artifact_id' "$manifest")
archive_sha=$(jq -er '.archive_sha256' "$manifest")
if [[ -n "${BROWSER_RUNTIME_ARCHIVE:-}" ]]; then
    # Reuse a locally retained copy with the same mandatory checksum validation.
    cp -- "$BROWSER_RUNTIME_ARCHIVE" "$temporary/runtime.zip"
elif ! gh api "repos/qqrm/openfga-axum-lab/actions/artifacts/$artifact/zip" >"$temporary/runtime.zip"; then
    # Preserve the exact source ZIP on Pages so the expiring Actions artifact
    # does not become the only copy required by future deployments.
    curl --fail --silent --show-error --location --retry 2 --max-time 300 \
        https://qqrm.github.io/openfga-axum-lab/wasm/runtime-source.zip \
        --output "$temporary/runtime.zip"
fi
printf '%s  %s\n' "$archive_sha" "$temporary/runtime.zip" | sha256sum --check --status
unzip -oq "$temporary/runtime.zip" -d "$output/runtime"
cp "$temporary/runtime.zip" "$output/runtime-source.zip"
jq -r '.files | to_entries[] | "\(.value.sha256)  \(.key)"' "$manifest" >"$temporary/files.sha256"
(cd "$output/runtime" && sha256sum --check "$temporary/files.sha256")
node browser-single/repack-runtime.mjs "$output/runtime" artifacts/browser-kernel
node browser-single/prepare-emulator.mjs "$output/runtime" artifacts/browser-emulator
node --check "$output/launcher.js"
node --check "$output/runtime-errors.mjs"
node --check "$output/runtime/out.js"
node --check "$output/isolation-sw.js"
bash -n "$output/guest.sh"
bash -n "$output/entrypoint.sh"
node --input-type=module - <<'JS'
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFileSync,writeFileSync} from 'node:fs';
const digest = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const build = JSON.parse(readFileSync('artifacts/browser-openfga/build.json','utf8'));
assert.equal(build.source_commit,'293893b22a53d8a130a5f7c97cab4bc8e91bcaa4');
assert.equal(build.upstream_version,'1.10.3');
assert.equal(build.startup_dial_timeout_seconds,60);
assert.equal(build.startup_dial_tests,'passed');
assert.equal(build.authorization_request_timeout_default_seconds,3);
assert.equal(build.startup_model_warmup,'in_process_codec_validation_only');
assert.equal(build.warmup_timeout_seconds,60);
assert.equal(build.warmup_cycles,4);
assert.equal(build.warmup_tests,'passed');
assert.equal(build.warmup_model_sha256,digest('fga/model.json'));
assert.equal(build.warmup_model_bytes,readFileSync('fga/model.json').length);
assert.equal(build.unary_raw_logging,'disabled_browser_only');
assert.equal(build.patch_sha256,digest('browser-single/openfga/startup-dial.patch'));
assert.equal(build.helper_sha256,digest('browser-single/openfga/browser_http_dial.go'));
assert.equal(build.tests_sha256,digest('browser-single/openfga/browser_http_dial_test.go'));
assert.equal(build.warmup_helper_sha256,digest('browser-single/openfga/browser_http_warmup.go'));
assert.equal(build.warmup_tests_sha256,digest('browser-single/openfga/browser_http_warmup_test.go'));
assert.equal(build.binary_sha256,digest('browser-output/wasm/runtime/openfga'));
const original = JSON.parse(readFileSync('browser-single/pages/runtime-manifest.json','utf8'));
const files = {};
for (const name of ['guest.sh','entrypoint.sh','runtime/openfga']) {
  const bytes = readFileSync(`browser-output/wasm/${name}`);
  files[name] = {bytes:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')};
}
const emulator = JSON.parse(readFileSync('browser-output/wasm/runtime/emulator-build/build.json','utf8'));
const startup = {base_source_commit:original.source_commit,openfga_build:build,emulator_build:emulator,files};
const startupPath = 'browser-output/wasm/runtime/startup-manifest.json';
writeFileSync(startupPath,JSON.stringify(startup,null,2)+'\n');
const derivedPath = 'browser-output/wasm/runtime/derived-runtime.json';
const derived = JSON.parse(readFileSync(derivedPath,'utf8'));
derived.startup_overlay = {manifest:'startup-manifest.json',sha256:digest(startupPath)};
derived.changes.push('Verified host-mount startup overlay: entrypoint and OpenFGA self-dial adaptation');
writeFileSync(derivedPath,JSON.stringify(derived,null,2)+'\n');
const path='browser-output/index.html';
const html=readFileSync(path,'utf8');
if(!html.includes('</main>'))throw new Error('Missing model explorer main element');
writeFileSync(path,html.replace('</main>','<section class="panel"><h2>Experimental browser Linux lab</h2><p>Run the real PostgreSQL, OpenFGA and Axum binaries locally in your tab. Large download; explicit opt-in; synthetic data only.</p><a href="wasm/">Open the browser experiment</a></section></main>'));
JS
printf 'Pinned browser runtime prepared; source: %s\n' "$(jq -r '.source_commit' "$manifest")"
