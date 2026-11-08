#!/usr/bin/env bash
# Bounded, explicitly requested manual testing only. No scheduled restart loop.
# Never expose a tunnel without --allowed-mail. Do not upload private tunnel logs.
set -euo pipefail
set +x
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

validate() {
  local email=${LIVE_LAB_ALLOWED_EMAIL:-} password=${LIVE_LAB_PASSWORD:-}
  local minutes=${LIVE_LAB_MINUTES:-30}
  # One mailbox only: no wildcard, comma, control character or option injection.
  if ! [[ "$email" =~ ^[A-Za-z0-9][A-Za-z0-9._+%-]*@[A-Za-z0-9][A-Za-z0-9.-]*\.[A-Za-z]{2,63}$ ]] \
    || test "${#email}" -gt 254; then
    echo 'Set LIVE_LAB_ALLOWED_EMAIL to one exact mailbox (no domain wildcards).' >&2
    return 1
  fi
  if test "${#password}" -lt 16 || test "${#password}" -gt 128 \
    || [[ "$password" == *$'\n'* || "$password" == *$'\r'* ]]; then
    echo 'Set LIVE_LAB_PASSWORD to 16-128 characters without line breaks.' >&2
    return 1
  fi
  case "$minutes" in 1|15|30|60) ;; *) echo 'Duration must be 15, 30 or 60 minutes (1 is reserved for verification).' >&2; return 1 ;; esac
}

protected_redirect() {
  local status=$1 location=$2
  case "$status" in 302|303|307|308) ;; *) return 1 ;; esac
  # Reject generic 403/5xx responses: they are not evidence of email protection.
  [[ "$location" == https://login.trycloudflare.com/* ]]
}

check_gate() {
  local url=$1 headers status location
  [[ "$url" =~ ^https://[a-z0-9]+(-[a-z0-9]+)*\.trycloudflare\.com$ ]] || return 1
  headers=$(mktemp)
  if ! status=$(curl --silent --show-error --noproxy '*' --connect-timeout 5 --max-time 10 \
    --dump-header "$headers" --output /dev/null --write-out '%{http_code}' "$url/readyz"); then
    rm -f "$headers"
    return 1
  fi
  location=$(awk 'tolower($1)=="location:" {sub(/\r$/, "", $2); print $2; exit}' "$headers")
  rm -f "$headers"
  protected_redirect "$status" "$location"
}

load_state() {
  : "${LIVE_LAB_STATE:?Missing owned session state}"
  : "${RUNNER_TEMP:?Hosted runner temporary directory is required}"
  [[ "$LIVE_LAB_STATE" == "$RUNNER_TEMP"/authzlab-live.* ]] || return 1
  test -d "$LIVE_LAB_STATE" && test ! -L "$LIVE_LAB_STATE"
  project=$(cat "$LIVE_LAB_STATE/project")
  [[ "$project" =~ ^authzlab-live-[0-9]+-[0-9]+$ ]] || return 1
  test "$project" = "authzlab-live-${GITHUB_RUN_ID:?}-${GITHUB_RUN_ATTEMPT:?}"
  export LAB_STATE_DIR="$LIVE_LAB_STATE/state"
  export APP_PORT=0
  compose=(docker compose -p "$project" --env-file "$LAB_STATE_DIR/environment.env" -f compose.release.yaml)
}

stop_session() {
  if test -z "${LIVE_LAB_STATE:-}" || ! test -d "$LIVE_LAB_STATE"; then return 0; fi
  load_state
  local pid
  if test -f "$LIVE_LAB_STATE/pid"; then
    pid=$(cat "$LIVE_LAB_STATE/pid")
    if [[ "$pid" =~ ^[0-9]+$ ]] && test -r "/proc/$pid/cmdline" \
      && grep -Fq "$LIVE_LAB_STATE/cloudflared" "/proc/$pid/cmdline"; then
      kill "$pid" 2>/dev/null || true
    fi
  fi
  if test -f "$LAB_STATE_DIR/environment.env"; then
    timeout 60 "${compose[@]}" down -v --remove-orphans -t 2
  fi
  rm -rf -- "$LIVE_LAB_STATE"
  if test -n "${GITHUB_STEP_SUMMARY:-}"; then
    printf '\n### Session ended\nThe tunnel and disposable lab have been stopped. Old URLs are not reusable.\n' >> "$GITHUB_STEP_SUMMARY"
  fi
}

start_session() {
  validate
  test "${GITHUB_ACTIONS:-false}" = true
  [[ "${GITHUB_RUN_ID:-}" =~ ^[0-9]+$ && "${GITHUB_RUN_ATTEMPT:-}" =~ ^[0-9]+$ ]]
  : "${RUNNER_TEMP:?}" "${GITHUB_ENV:?}" "${GITHUB_SHA:?}" "${GITHUB_STEP_SUMMARY:?}"
  umask 077
  LIVE_LAB_STATE=$(mktemp -d "$RUNNER_TEMP/authzlab-live.XXXXXX")
  export LIVE_LAB_STATE
  printf 'LIVE_LAB_STATE=%s\n' "$LIVE_LAB_STATE" >> "$GITHUB_ENV"
  printf 'authzlab-live-%s-%s\n' "$GITHUB_RUN_ID" "$GITHUB_RUN_ATTEMPT" > "$LIVE_LAB_STATE/project"
  trap 'if test "$?" -ne 0; then stop_session || true; fi' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  load_state
  local tag image origin url='' pid expiry
  tag="authzlab-live:$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT"
  docker build --build-arg "APP_REVISION=$GITHUB_SHA" -t "$tag" .
  image=$(docker image inspect "$tag" --format '{{.Id}}')
  docker run --rm --user "$(id -u):$(id -g)" -v "$LIVE_LAB_STATE:$LIVE_LAB_STATE" \
    --entrypoint /usr/local/bin/lab-tool "$image" init-env "$LAB_STATE_DIR" "$image"
  chmod 600 "$LAB_STATE_DIR/secrets/demo-password"
  printf '%s' "$LIVE_LAB_PASSWORD" > "$LAB_STATE_DIR/secrets/demo-password"
  chmod 444 "$LAB_STATE_DIR/secrets/demo-password"
  "${compose[@]}" up -d --wait --wait-timeout 120 app
  "${compose[@]}" run --rm --no-deps acceptance
  origin=$("${compose[@]}" port app 3000)
  [[ "$origin" =~ ^127\.0\.0\.1:[0-9]+$ ]]
  printf 'http://%s\n' "$origin" > "$LIVE_LAB_STATE/origin"
  curl --fail --silent --show-error --max-time 5 "http://$origin/version" > "$LIVE_LAB_STATE/version.json"
  test "$(jq -r .revision "$LIVE_LAB_STATE/version.json")" = "$GITHUB_SHA"
  # Official release 2026.10.7, verified against GitHub asset digest. No auto-update.
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
    'https://github.com/cloudflare/cloudflared/releases/download/2026.10.7/cloudflared-linux-amd64' \
    --output "$LIVE_LAB_STATE/cloudflared"
  printf '1f99fb518b83dec2ba8d870a7cc991d82a77e6196295a3d2754ba17ab9f91872  %s\n' "$LIVE_LAB_STATE/cloudflared" | sha256sum --check --status
  chmod 700 "$LIVE_LAB_STATE/cloudflared"
  "$LIVE_LAB_STATE/cloudflared" tunnel --help > "$LIVE_LAB_STATE/help"
  grep -q -- '--allowed-mail' "$LIVE_LAB_STATE/help"
  mkdir "$LIVE_LAB_STATE/home"
  # An empty environment prevents forwarding Actions credentials into cloudflared.
  nohup env -i PATH="$PATH" HOME="$LIVE_LAB_STATE/home" \
    "$LIVE_LAB_STATE/cloudflared" tunnel --no-autoupdate --protocol http2 \
    --url "http://$origin" --allowed-mail "$LIVE_LAB_ALLOWED_EMAIL" \
    > "$LIVE_LAB_STATE/tunnel.log" 2>&1 < /dev/null &
  pid=$!
  printf '%s\n' "$pid" > "$LIVE_LAB_STATE/pid"
  for _ in {1..60}; do
    kill -0 "$pid"
    url=$(grep -m1 -Eo 'https://[a-z0-9]+(-[a-z0-9]+)*\.trycloudflare\.com' "$LIVE_LAB_STATE/tunnel.log" || true)
    if test -n "$url" && check_gate "$url"; then break; fi
    sleep 2
  done
  test -n "$url"
  check_gate "$url" || { echo 'Protected email login was not confirmed; stopping, never falling back to a public tunnel.' >&2; return 1; }
  expiry=$(( $(date +%s) + ${LIVE_LAB_MINUTES:-30} * 60 ))
  printf '%s\n' "$expiry" > "$LIVE_LAB_STATE/expires"
  printf '%s\n' "$url" > "$LIVE_LAB_STATE/url"
  printf '::notice title=Open private live lab::%s (expires %s UTC; allowed email PIN, then your LIVE_LAB_PASSWORD)\n' "$url" "$(date -u -d "@$expiry" +'%Y-%m-%d %H:%M:%S')"
  {
    printf '## Temporary real laboratory\n\n[Open the protected lab](%s)\n\n' "$url"
    printf 'Expires: **%s UTC**. Valid only while **Keep the session available** is running.\n\n' "$(date -u -d "@$expiry" +'%Y-%m-%d %H:%M:%S')"
    printf 'Enter the configured mailbox and email PIN, then log in as alice/bob/carol/dave with your LIVE_LAB_PASSWORD. Credentials are not published.\n\n'
    printf 'Source: %s. Real Axum + PostgreSQL + OpenFGA; local authorization acceptance passed; unauthenticated request redirected to email login.\n' "$GITHUB_SHA"
  } >> "$GITHUB_STEP_SUMMARY"
  mkdir -p artifacts
  jq -n --arg url "$url" --arg sha "$GITHUB_SHA" --arg image "$image" --argjson expires "$expiry" \
    '{source:$sha,image:$image,url:$url,expires_unix:$expires,local_authorization_verified:true,unauthenticated_email_gate_verified:true,authenticated_browser_login_verified:false}' > artifacts/live-lab.json
  trap - EXIT INT TERM
}

wait_session() {
  load_state
  local url origin pid expiry now remaining
  url=$(cat "$LIVE_LAB_STATE/url")
  origin=$(cat "$LIVE_LAB_STATE/origin")
  pid=$(cat "$LIVE_LAB_STATE/pid")
  expiry=$(cat "$LIVE_LAB_STATE/expires")
  [[ "$pid" =~ ^[0-9]+$ && "$expiry" =~ ^[0-9]+$ ]]
  trap 'stop_session' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  while true; do
    now=$(date +%s)
    if test "$now" -ge "$expiry"; then break; fi
    kill -0 "$pid"
    curl --fail --silent --show-error --max-time 5 "$origin/readyz" > /dev/null
    check_gate "$url" || { echo 'Email protection check failed; closing the session.' >&2; return 1; }
    remaining=$((expiry-now))
    if test "$remaining" -gt 30; then remaining=30; fi
    sleep "$remaining"
  done
}

case "${1:-}" in
  validate) validate ;;
  classify-gate) test "$#" = 3; protected_redirect "$2" "$3" ;;
  check-gate) test "$#" = 2; check_gate "$2" ;;
  start) start_session ;;
  wait) wait_session ;;
  stop) stop_session ;;
  *) echo 'usage: live-lab.sh validate|start|wait|stop|check-gate URL' >&2; exit 2 ;;
esac
