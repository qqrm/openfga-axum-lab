#!/usr/bin/env bash
# One *ephemeral* Linux container: real PostgreSQL + OpenFGA + Rust/Axum.
# This is intentionally not a secure or durable production composition.
set -Eeuo pipefail
umask 077

if [[ "${BROWSER_LAB_EPHEMERAL:-}" != "yes" ]]; then
    echo "Refusing to start the trust-authenticated, ephemeral browser lab." >&2
    echo "Set BROWSER_LAB_EPHEMERAL=yes only in an isolated sandbox." >&2
    exit 64
fi
if [[ "$(id -un)" != "postgres" ]]; then
    echo "This experiment must run as the unprivileged postgres user." >&2
    exit 64
fi

runtime_dir=/tmp/browser-lab
export PGDATA="$runtime_dir/postgres"
export PGHOST=127.0.0.1 PGPORT=5432 PGUSER=authzlab
export DATABASE_URL="postgres://authzlab@127.0.0.1:5432/authzlab?sslmode=disable"
export OPENFGA_DATASTORE_ENGINE=postgres
export OPENFGA_DATASTORE_URI="postgres://authzlab@127.0.0.1:5432/openfga?sslmode=disable&host=/tmp/browser-lab"
export OPENFGA_DATASTORE_MAX_OPEN_CONNS=10
export OPENFGA_HTTP_ADDR=127.0.0.1:8080
export OPENFGA_GRPC_ADDR=127.0.0.1:8081
export OPENFGA_PLAYGROUND_ENABLED=false
export FGA_URL=http://127.0.0.1:8080 FGA_HTTP_URL=http://127.0.0.1:8080
export FGA_CONFIG="$runtime_dir/config.json" FGA_MODEL=/app/fga/model.json
export DEMO_PASSWORD_FILE="$runtime_dir/demo-password"
export DEMO_SEED=true DEMO_MIGRATE=true
export LISTEN_ADDR=0.0.0.0:3000
# Pages supplies a reviewed browser-only build through the existing host mount.
# The native image continues to use the checksummed official release binary.
openfga_binary=${BROWSER_LAB_OPENFGA_BINARY:-/usr/local/bin/openfga}
if [[ ! -x "$openfga_binary" ]]; then
    echo "OpenFGA executable is unavailable: $openfga_binary" >&2
    exit 66
fi
# The browser overlay selects only initial bootstrap. Native startup keeps the
# original CLI and its default model budget unless explicitly configured.
bootstrap_binary=${BROWSER_LAB_BOOTSTRAP_BINARY:-/usr/local/bin/lab-tool}
if [[ ! -x "$bootstrap_binary" ]]; then
    echo "Bootstrap executable is unavailable: $bootstrap_binary" >&2
    exit 66
fi

mkdir -p "$runtime_dir"
chmod 0700 "$runtime_dir"
if [[ -e "$PGDATA" || -e "$FGA_CONFIG" || -e "$DEMO_PASSWORD_FILE" ]]; then
    echo "Refusing to reuse an existing ephemeral cluster or credentials." >&2
    exit 65
fi

postgres_started=false
fga_pid=
app_pid=

stack_phase_name=entrypoint-setup
stack_exit_reason=entrypoint-command-failed
stack_process_observed=not-started
stack_process_state() {
    local pid=$1 key value
    stack_process_observed=not-started
    if [[ ! "$pid" =~ ^[1-9][0-9]*$ ]]; then return 0; fi
    stack_process_observed=exited
    if kill -0 "$pid" 2>/dev/null; then
        stack_process_observed=unknown
        if [[ -r "/proc/$pid/status" ]]; then
            while read -r key value _; do
                if [[ "$key" == State: && "$value" =~ ^[A-Z]$ ]]; then
                    stack_process_observed=$value
                    break
                fi
            done 2>/dev/null <"/proc/$pid/status" || true
        fi
    fi
}
stack_phase() {
    stack_phase_name=$1
    local uptime_seconds=unavailable
    read -r uptime_seconds _ </proc/uptime || true
    printf 'STACK|phase|{"phase":"%s","datastore":"%s","guest_seconds":%s,"uptime_seconds":"%s","clock_scope":"entrypoint"}\n' \
        "$stack_phase_name" "$OPENFGA_DATASTORE_ENGINE" "$SECONDS" "$uptime_seconds" || true
}
stack_entrypoint_exit() {
    local exit_code=$1 uptime_seconds=unavailable observed_app_pid=0 observed_fga_pid=0 app_state fga_state
    read -r uptime_seconds _ </proc/uptime || true
    if [[ -n "$app_pid" ]]; then observed_app_pid=$app_pid; fi
    if [[ -n "$fga_pid" ]]; then observed_fga_pid=$fga_pid; fi
    stack_process_state "$observed_app_pid"
    app_state=$stack_process_observed
    stack_process_state "$observed_fga_pid"
    fga_state=$stack_process_observed
    printf 'STACK|entrypoint-exit|{"reason":"%s","phase":"%s","datastore":"%s","exit_code":%s,"guest_seconds":%s,"uptime_seconds":"%s","clock_scope":"entrypoint","app_pid":%s,"fga_pid":%s,"app_state":"%s","fga_state":"%s"}\n' \
        "$stack_exit_reason" "$stack_phase_name" "$OPENFGA_DATASTORE_ENGINE" "$exit_code" "$SECONDS" "$uptime_seconds" \
        "$observed_app_pid" "$observed_fga_pid" "$app_state" "$fga_state" || true
}
# cleanup is registered as an EXIT/INT/TERM handler, not called directly.
# ShellCheck 0.10 reports the indirect trap body as SC2317; 0.11 uses SC2329.
# shellcheck disable=SC2317,SC2329
cleanup() {
    status=$?
    trap - EXIT INT TERM
    stack_entrypoint_exit "$status" || true
    for pid in "$app_pid" "$fga_pid"; do
        if [[ -n "$pid" ]]; then
            kill -TERM "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
    if [[ "$postgres_started" == true ]]; then
        pg_ctl -D "$PGDATA" -m fast -w stop >/dev/null 2>&1 || true
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'stack_exit_reason=signal-int; exit 130' INT
trap 'stack_exit_reason=signal-term; exit 143' TERM

echo "[browser-single] initializing isolated PostgreSQL 17"
# Apply the runtime budget during initialization; initdb still runs real probes.
stack_phase initdb-start
initdb -D "$PGDATA" -U "$PGUSER" --auth=trust --encoding=UTF8 --no-instructions \
    --set=max_connections=60 --set=shared_buffers=16MB \
    >"$runtime_dir/initdb.log" 2>&1 || { cat "$runtime_dir/initdb.log" >&2; exit 1; }
stack_phase initdb-complete
stack_phase postgres-start
pg_ctl -D "$PGDATA" -l "$runtime_dir/postgres.log" \
    -o "-c listen_addresses=127.0.0.1 -c unix_socket_directories=$runtime_dir -c shared_buffers=16MB -c max_connections=60" \
    -w start || { cat "$runtime_dir/postgres.log" >&2; exit 1; }
postgres_started=true
stack_phase postgres-ready
stack_phase createdb-openfga-start
createdb -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" openfga
stack_phase createdb-openfga-complete
stack_phase createdb-authzlab-start
createdb -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" authzlab
stack_phase createdb-authzlab-complete

echo "[browser-single] migrating and starting real OpenFGA 1.10.3"
stack_phase openfga-migrate-start
"$openfga_binary" migrate
stack_phase openfga-migrate-complete
stack_phase openfga-start
"$openfga_binary" run &
fga_pid=$!
stack_phase openfga-started

# Do not spend bootstrap's existing 60-second readiness budget while Go is
# still initializing. A listening socket only schedules bootstrap: its real
# /healthz SERVING and pinned-model checks remain the readiness authority.
# Allow the browser build's bounded 60-second codec warmup before HTTP starts.
echo "[browser-single] waiting for the OpenFGA HTTP listener"
stack_phase openfga-listener-start
http_listening=false
deadline=$((SECONDS + 180))
while (( SECONDS < deadline )); do
    if ! kill -0 "$fga_pid" 2>/dev/null; then
        stack_exit_reason=openfga-exited-before-listener
        echo "[browser-single] OpenFGA exited before its HTTP listener started" >&2
        exit 1
    fi
    if timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/8080' 2>/dev/null; then
        http_listening=true
        break
    fi
    sleep 3
done
if [[ "$http_listening" != true ]]; then
    stack_exit_reason=openfga-listener-deadline
    echo "[browser-single] OpenFGA HTTP listener startup timed out" >&2
    exit 1
fi

stack_phase openfga-listener-ready
echo "[browser-single] binding the pinned Lakekeeper model and seeding real SQL data"
stack_phase bootstrap-start
"$bootstrap_binary" bootstrap
stack_phase bootstrap-complete

# Never put the password in the image, workflow logs, command line or git.
od -An -tx1 -N24 /dev/urandom | tr -d '[:space:]' >"$DEMO_PASSWORD_FILE"
chmod 0600 "$DEMO_PASSWORD_FILE"

echo "[browser-single] starting real Axum, HTTP port 3000"
stack_phase app-start
/usr/local/bin/app &
app_pid=$!
stack_phase app-started

# A daemon exiting unexpectedly must terminate the experiment, not leave a
# silently degraded static demo behind.
stack_phase app-wait
set +e
wait -n "$app_pid" "$fga_pid"
status=$?
set -e
stack_exit_reason=required-service-exited
stack_phase app-wait-finished
echo "[browser-single] a required service exited (status $status)" >&2
if (( status == 0 )); then
    exit 1
fi
exit "$status"
