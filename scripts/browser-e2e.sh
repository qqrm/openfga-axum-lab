#!/usr/bin/env bash
# Execute the browser against an isolated real Rust/PostgreSQL/OpenFGA stack.
set -euo pipefail
: "${DEMO_PASSWORD:?set DEMO_PASSWORD for the disposable test stand}"
export POSTGRES_PASSWORD="$DEMO_PASSWORD"
export APP_PORT=0
export FGA_TRANSPORT=http
export FGA_APP_URL=http://openfga:8080
project="authzlab-browser-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$BASHPID"
repo_dir=$(pwd)
compose=(docker compose -p "$project" -f "$repo_dir/compose.yaml")
cleanup() { "${compose[@]}" down -v -t 2; }
trap cleanup EXIT
"${compose[@]}" up -d --build app
address=$("${compose[@]}" port app 3000)
[[ "$address" == 127.0.0.1:* ]] || { echo 'Browser test API must bind to loopback'; exit 1; }
export BASE_URL="http://$address"
ready=false
for ((attempt=0; attempt<180; attempt++)); do
  if curl --fail --silent --max-time 3 "$BASE_URL/readyz" | jq -e '.ready == true' > /dev/null; then
    ready=true
    break
  fi
  sleep 1
done
[[ "$ready" == true ]] || { echo 'Real stack did not pass readiness'; exit 1; }
cd e2e
npm test
