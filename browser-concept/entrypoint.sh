#!/usr/bin/env bash
# Real PostgreSQL document/session storage, OpenFGA memory, and Rust/Axum.
# The Lakekeeper application and its migration contract are not invoked here.
set -Eeuo pipefail
umask 077

phase() {
    printf 'CONCEPT|phase|{"phase":"%s","detail":"%s"}\n' "$1" "$2"
}
fail() {
    printf 'CONCEPT|failed|{"message":"%s"}\n' "$1"
    exit 1
}
[[ "${BROWSER_CONCEPT_EPHEMERAL:-}" == yes ]] || fail 'Explicit ephemeral guest startup is required.'
[[ "$(id -un)" == postgres ]] || fail 'An unprivileged service account is required.'

runtime_dir=/tmp/browser-concept
executable_dir=${CONCEPT_RUNTIME_DIR:-/tmp/browser-concept-runtime}
concept_binary="$executable_dir/concept"
openfga_binary="$executable_dir/openfga"
[[ -x "$concept_binary" && -x "$openfga_binary" ]] || fail 'A verified executable is unavailable.'
[[ ! -e "$runtime_dir" ]] || fail 'A previous concept runtime cannot be reused.'
mkdir -m 0700 "$runtime_dir"

export PGDATA="$runtime_dir/postgres"
export PGHOST=127.0.0.1 PGPORT=5432 PGUSER=authzlab PGCONNECT_TIMEOUT=10
export DATABASE_URL='postgres://authzlab@127.0.0.1:5432/concept?sslmode=disable'
export OPENFGA_DATASTORE_ENGINE=memory
export OPENFGA_HTTP_ADDR=127.0.0.1:8080 OPENFGA_GRPC_ADDR=127.0.0.1:8081
export OPENFGA_PLAYGROUND_ENABLED=false OPENFGA_METRICS_ENABLED=false OPENFGA_LOG_LEVEL=warn
export OPENFGA_REQUEST_TIMEOUT=3s
export FGA_URL=http://127.0.0.1:8080
export CONCEPT_LISTEN_ADDR=127.0.0.1:3000 CONCEPT_BASE_URL=http://127.0.0.1:3000
export CONCEPT_CREDENTIALS_FILE="$runtime_dir/credentials.json"

postgres_pid=
fga_pid=
app_pid=
bridge_pid=
# These functions are signal/EXIT callbacks rather than direct calls.
# shellcheck disable=SC2317,SC2329
cleanup() {
    local status=$?
    trap - EXIT INT TERM ERR
    local pid
    for pid in "$bridge_pid" "$app_pid" "$fga_pid" "$postgres_pid"; do
        if [[ -n "$pid" ]]; then kill -TERM "$pid" 2>/dev/null || true; fi
    done
    for pid in "$bridge_pid" "$app_pid" "$fga_pid" "$postgres_pid"; do
        if [[ -n "$pid" ]]; then wait "$pid" 2>/dev/null || true; fi
    done
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'fail "The concept runtime stopped during setup."' ERR

phase postgres-init 'Initializing the document and session database.'
if ! initdb -D "$PGDATA" -U "$PGUSER" --auth=trust --encoding=UTF8 --no-instructions \
    --set=max_connections=60 --set=shared_buffers=16MB >"$runtime_dir/initdb.log" 2>&1; then
    tail -c 4000 "$runtime_dir/initdb.log"
    fail 'PostgreSQL initialization failed.'
fi
phase postgres-start 'Starting PostgreSQL inside this tab.'
# Keep the real server as a child so wait -n detects its exit immediately.
postgres -D "$PGDATA" -c listen_addresses=127.0.0.1 \
    -c "unix_socket_directories=$runtime_dir" -c shared_buffers=16MB -c max_connections=60 \
    >"$runtime_dir/postgres.log" 2>&1 &
postgres_pid=$!
postgres_ready=false
deadline=$((SECONDS + 90))
while (( SECONDS < deadline )); do
    kill -0 "$postgres_pid" 2>/dev/null || break
    if pg_isready -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" -d postgres >/dev/null 2>&1; then
        postgres_ready=true
        break
    fi
    sleep 3
done
if [[ "$postgres_ready" != true ]]; then
    tail -c 4000 "$runtime_dir/postgres.log"
    fail 'PostgreSQL did not become ready.'
fi
createdb -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" concept
phase postgres-ready 'The real document and session database is ready.'

phase openfga-start 'Starting the authorization engine.'
"$openfga_binary" run &
fga_pid=$!
fga_listening=false
deadline=$((SECONDS + 180))
while (( SECONDS < deadline )); do
    kill -0 "$fga_pid" 2>/dev/null || break
    kill -0 "$postgres_pid" 2>/dev/null || break
    if timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/8080' 2>/dev/null; then
        fga_listening=true
        break
    fi
    sleep 3
done
[[ "$fga_listening" == true ]] || fail 'OpenFGA did not open its listener.'
phase application-start 'Loading the small policy and starting the Rust HTTP API.'
"$concept_binary" serve &
app_pid=$!

# The bridge obtains readiness through the real HTTP API and keeps the two
# server-provisioned synthetic credentials inside the guest. Explicit stdin
# redirection preserves the UART for this background child.
"$concept_binary" bridge <&0 &
bridge_pid=$!
set +e
wait -n "$postgres_pid" "$fga_pid" "$app_pid" "$bridge_pid"
status=$?
set -e
printf '[concept-guest] a required process exited with status %s\n' "$status"
fail 'A required concept service or its HTTP bridge stopped.'
