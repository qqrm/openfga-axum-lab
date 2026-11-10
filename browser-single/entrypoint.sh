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
export OPENFGA_DATASTORE_URI="postgres://authzlab@127.0.0.1:5432/openfga?sslmode=disable"
export OPENFGA_DATASTORE_MAX_OPEN_CONNS=10
export OPENFGA_HTTP_ADDR=127.0.0.1:8080
export OPENFGA_GRPC_ADDR=127.0.0.1:8081
export OPENFGA_PLAYGROUND_ENABLED=false
export FGA_URL=http://127.0.0.1:8080 FGA_HTTP_URL=http://127.0.0.1:8080
export FGA_CONFIG="$runtime_dir/config.json" FGA_MODEL=/app/fga/model.json
export DEMO_PASSWORD_FILE="$runtime_dir/demo-password"
export DEMO_SEED=true DEMO_MIGRATE=true
export LISTEN_ADDR=0.0.0.0:3000

mkdir -p "$runtime_dir"
chmod 0700 "$runtime_dir"
if [[ -e "$PGDATA" || -e "$FGA_CONFIG" || -e "$DEMO_PASSWORD_FILE" ]]; then
    echo "Refusing to reuse an existing ephemeral cluster or credentials." >&2
    exit 65
fi

postgres_started=false
fga_pid=
app_pid=
cleanup() {
    status=$?
    trap - EXIT INT TERM
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
trap 'exit 130' INT
trap 'exit 143' TERM

echo "[browser-single] initializing isolated PostgreSQL 17"
initdb -D "$PGDATA" -U "$PGUSER" --auth=trust --encoding=UTF8 --no-instructions \
    >"$runtime_dir/initdb.log" 2>&1 || { cat "$runtime_dir/initdb.log" >&2; exit 1; }
pg_ctl -D "$PGDATA" -l "$runtime_dir/postgres.log" \
    -o "-c listen_addresses=127.0.0.1 -c unix_socket_directories=$runtime_dir -c shared_buffers=16MB -c max_connections=60" \
    -w start || { cat "$runtime_dir/postgres.log" >&2; exit 1; }
postgres_started=true
createdb -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" openfga
createdb -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" authzlab

echo "[browser-single] migrating and starting real OpenFGA 1.10.3"
/usr/local/bin/openfga migrate
/usr/local/bin/openfga run &
fga_pid=$!

echo "[browser-single] binding the pinned Lakekeeper model and seeding real SQL data"
/usr/local/bin/lab-tool bootstrap

# Never put the password in the image, workflow logs, command line or git.
od -An -tx1 -N24 /dev/urandom | tr -d '[:space:]' >"$DEMO_PASSWORD_FILE"
chmod 0600 "$DEMO_PASSWORD_FILE"

echo "[browser-single] starting real Axum, HTTP port 3000"
/usr/local/bin/app &
app_pid=$!

# A daemon exiting unexpectedly must terminate the experiment, not leave a
# silently degraded static demo behind.
set +e
wait -n "$app_pid" "$fga_pid"
status=$?
set -e
echo "[browser-single] a required service exited (status $status)" >&2
if (( status == 0 )); then
    exit 1
fi
exit "$status"
