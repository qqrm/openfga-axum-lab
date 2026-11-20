#!/usr/bin/env bash
# Component diagnostic only. This never starts PostgreSQL or the Rust application.
set -Eeuo pipefail
export LC_ALL=C
umask 077

http_request() {
    local request_path=$1 body=$2 output=$3
    local limit=16384 bytes header_bytes=0 line version code header key value
    local length='' content_type='' header_done=false
    [[ "$request_path" =~ ^/stores/[0-9A-HJKMNP-TV-Z]{26}/(check|write)$ ]] || return 64
    [[ ${#body} -le 4096 && "$output" == /tmp/fga-memory/response-* ]] || return 64
    : >"$output"
    # One external timeout covers connection, send, complete EOF and framing.
    exec 3<>/dev/tcp/127.0.0.1/8080
    printf 'POST %s HTTP/1.0\r\nHost: 127.0.0.1:8080\r\nContent-Type: application/json\r\nContent-Length: %s\r\nConnection: close\r\n\r\n%s' \
        "$request_path" "${#body}" "$body" >&3
    # dd writes each received block immediately, preserving partial evidence if
    # the enclosing timeout fires before EOF. Do not use iflag=fullblock here.
    dd bs=4096 count="$((limit + 1))" iflag=count_bytes status=none <&3 >"$output"
    exec 3<&- 3>&-
    bytes=$(wc -c <"$output")
    (( bytes > 0 && bytes <= limit )) || return 65
    exec 4<"$output"
    IFS= read -r line <&4 || return 65
    header_bytes=$((${#line} + 1))
    [[ "$line" == *$'\r' ]] || return 65
    line=${line%$'\r'}
    [[ "$line" =~ ^(HTTP/1\.[01])\ ([0-9]{3})(\ .*)?$ ]] || return 65
    version=${BASH_REMATCH[1]}
    code=${BASH_REMATCH[2]}
    [[ "$code" != 1?? ]] || return 65
    while IFS= read -r line <&4; do
        header_bytes=$((header_bytes + ${#line} + 1))
        [[ "$line" == *$'\r' ]] || return 65
        header=${line%$'\r'}
        if [[ -z "$header" ]]; then header_done=true; break; fi
        [[ "$header" =~ ^([A-Za-z0-9-]+):[[:space:]]*(.*)$ ]] || return 65
        key=${BASH_REMATCH[1],,}
        value=${BASH_REMATCH[2]}
        case "$key" in
            content-length)
                [[ -z "$length" && "$value" =~ ^[0-9]{1,5}$ ]] || return 65
                length=$((10#$value)) ;;
            transfer-encoding) return 65 ;;
            content-encoding) return 65 ;;
            content-type)
                [[ -z "$content_type" ]] || return 65
                content_type=$value ;;
        esac
    done
    exec 4<&-
    [[ "$header_done" == true && "$content_type" == application/json* ]] || return 65
    (( header_bytes <= bytes )) || return 65
    if [[ -n "$length" ]]; then
        (( length == bytes - header_bytes )) || return 65
    else
        # HTTP/1.0 close-delimited JSON is valid only after the actual EOF above.
        [[ "$version" == HTTP/1.0 ]] || return 65
    fi
}

process_snapshot() {
    local snapshot_phase=$1 server_pid=$2 elapsed_seconds=$3
    local output=/tmp/fga-memory/processes.tsv proc pid comm key value parent exe
    local records=0 forbidden=0
    [[ "$snapshot_phase" =~ ^(before-bootstrap|after-cycles|failure)$ ]] || return 64
    : >"$output"
    for proc in /proc/[0-9]*; do
        pid=${proc##*/}
        IFS= read -r comm <"$proc/comm" 2>/dev/null || continue
        parent=unavailable
        while IFS=: read -r key value; do
            if [[ "$key" == PPid ]]; then parent=${value//[[:space:]]/}; break; fi
        done <"$proc/status" 2>/dev/null || true
        if [[ ! "$parent" =~ ^[0-9]+$ ]]; then
            [[ ! -d "$proc" ]] && continue
            return 65
        fi
        exe=-
        if [[ "$pid" == "$server_pid" ]]; then exe=$(readlink "$proc/exe" 2>/dev/null || printf unavailable); fi
        printf '%s\t%s\t%s\t%s\n' "$pid" "$parent" "$comm" "$exe" >>"$output"
        records=$((records + 1))
        (( records <= 256 )) || return 65
        case "$comm" in postgres|initdb|pg_ctl|createdb|psql|app) forbidden=$((forbidden + 1)) ;; esac
    done
    printf 'FGA_MEMORY|processes|{"phase":"%s","snapshot_b64":"%s","records":%s,"forbidden_count":%s,"openfga_pid":%s,"guest_seconds":%s}\n' \
        "$snapshot_phase" "$(base64 -w0 "$output")" "$records" "$forbidden" "$server_pid" "$elapsed_seconds"
    (( forbidden == 0 ))
}

if [[ ${1:-} == http-request ]]; then
    [[ $# == 4 ]] || exit 64
    http_request "$2" "$3" "$4"
    exit "$?"
fi
if [[ ${1:-} == process-snapshot ]]; then
    [[ $# == 4 ]] || exit 64
    process_snapshot "$2" "$3" "$4"
    exit "$?"
fi
[[ $# == 0 ]] || exit 64

readonly model_sha256=a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d
phase=boot
fga_pid=0
idle() { while :; do sleep 60; done; }
failure() {
    local status=$1 reason=$2 uptime_seconds=unavailable diagnostics_status=0
    trap - ERR
    set +e
    read -r uptime_seconds _ </proc/uptime
    printf 'FGA_MEMORY|failed|{"reason":"%s","phase":"%s","exit_code":%s,"guest_seconds":%s,"uptime_seconds":"%s"}\n' \
        "$reason" "$phase" "$status" "$SECONDS" "$uptime_seconds"
    if [[ -d /tmp/fga-memory ]]; then
        timeout --kill-after=1s 5s /bin/bash "$0" process-snapshot failure "$fga_pid" "$SECONDS" || diagnostics_status=$?
    fi
    read -r uptime_seconds _ </proc/uptime
    printf 'FGA_MEMORY|stage|{"phase":"failure-diagnostics-finished","diagnostics_exit":%s,"guest_seconds":%s,"uptime_seconds":"%s"}\n' \
        "$diagnostics_status" "$SECONDS" "$uptime_seconds"
    idle
}
trap 'failure "$?" unexpected-command-failure' ERR
stage() {
    phase=$1
    local uptime_seconds=unavailable
    read -r uptime_seconds _ </proc/uptime
    printf 'FGA_MEMORY|stage|{"phase":"%s","guest_seconds":%s,"uptime_seconds":"%s"}\n' \
        "$phase" "$SECONDS" "$uptime_seconds"
}

printf '\nFGA_MEMORY|boot|{"datastore":"memory","virtual_cpus":2,"guest_seconds":%s}\n' "$SECONDS"
for tool in timeout dd wc base64 sha256sum cp chmod readlink tee sleep cat mkdir; do
    command -v "$tool" >/dev/null || failure 69 missing-required-runtime-tool
done
online_cpus=
read -r online_cpus </sys/devices/system/cpu/online
allowed_cpus=
while IFS=: read -r key value; do
    if [[ "$key" == Cpus_allowed_list ]]; then allowed_cpus=${value//[[:space:]]/}; break; fi
done </proc/self/status
printf 'FGA_MEMORY|cpu|{"online_cpus":"%s","allowed_cpus":"%s","guest_seconds":%s}\n' \
    "$online_cpus" "$allowed_cpus" "$SECONDS"
[[ "$online_cpus" == 0-1 && "$allowed_cpus" == 0-1 ]] || failure 65 unexpected-guest-cpu-topology
mkdir -m 0700 /tmp/fga-memory
mkdir -m 0700 /tmp/browser-lab-runtime
stage executable-staging
timeout --kill-after=5s 900s cp /browser-host/openfga /browser-host/bootstrap /tmp/browser-lab-runtime/
chmod 0500 /tmp/browser-lab-runtime/openfga /tmp/browser-lab-runtime/bootstrap
printf '%s  /app/fga/model.json\n' "$model_sha256" | sha256sum --check --status

export TZ=UTC
export OPENFGA_DATASTORE_ENGINE=memory
unset OPENFGA_DATASTORE_URI OPENFGA_DATASTORE_SECONDARY_URI OPENFGA_DATASTORE_USERNAME OPENFGA_DATASTORE_PASSWORD
export OPENFGA_HTTP_ADDR=127.0.0.1:8080 OPENFGA_GRPC_ADDR=127.0.0.1:8081
export OPENFGA_PLAYGROUND_ENABLED=false OPENFGA_METRICS_ENABLED=false OPENFGA_LOG_LEVEL=warn
export FGA_URL=http://127.0.0.1:8080 FGA_CONFIG=/tmp/fga-memory/config.json FGA_MODEL=/app/fga/model.json
stage openfga-start
/tmp/browser-lab-runtime/openfga run > >(tee /tmp/fga-memory/openfga.log) 2>&1 &
fga_pid=$!
stage listener-wait
deadline=$((SECONDS + 180))
listening=false
while (( SECONDS < deadline )); do
    kill -0 "$fga_pid" 2>/dev/null || failure 1 openfga-exited-before-listener
    if timeout 3s /bin/bash -c 'exec 3<>/dev/tcp/127.0.0.1/8080' 2>/dev/null; then listening=true; break; fi
    sleep 3
done
[[ "$listening" == true ]] || failure 124 listener-startup-deadline
timeout --kill-after=1s 5s /bin/bash "$0" process-snapshot before-bootstrap "$fga_pid" "$SECONDS"
stage bootstrap
BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS=30 BROWSER_BOOTSTRAP_EMPTY_STORE_CHECK=yes \
    /tmp/browser-lab-runtime/bootstrap bootstrap
config_bytes=$(wc -c <"$FGA_CONFIG")
(( config_bytes > 0 && config_bytes <= 256 )) || failure 65 invalid-bootstrap-config-size
config=$(cat "$FGA_CONFIG")
[[ ${#config} -eq "$config_bytes" ]] || failure 65 noncanonical-bootstrap-config-bytes
config_pattern='^\{"store_id":"([0-9A-HJKMNP-TV-Z]{26})","model_id":"([0-9A-HJKMNP-TV-Z]{26})","model_sha256":"([0-9a-f]{64})"\}$'
[[ "$config" =~ $config_pattern ]] || failure 65 invalid-bootstrap-config
store_id=${BASH_REMATCH[1]}
model_id=${BASH_REMATCH[2]}
[[ ${BASH_REMATCH[3]} == "$model_sha256" ]] || failure 65 bootstrap-model-hash-mismatch
printf 'FGA_MEMORY|bootstrap|{"config_b64":"%s","store_id":"%s","model_id":"%s","model_sha256":"%s","guest_seconds":%s}\n' \
    "$(base64 -w0 "$FGA_CONFIG")" "$store_id" "$model_id" "$model_sha256" "$SECONDS"

requests=0
for cycle in 1 2; do
    user=user:browser-memory-probe-$cycle
    object=lakekeeper_table:browser-memory-probe-$cycle
    tuple=$(printf '{"user":"%s","relation":"ownership","object":"%s"}' "$user" "$object")
    for operation in deny-before grant allow revoke deny-after; do
        stage "cycle-$cycle-$operation"
        case "$operation" in
            grant)
                endpoint='write'
                body=$(printf '{"authorization_model_id":"%s","writes":{"tuple_keys":[%s]}}' "$model_id" "$tuple") ;;
            revoke)
                endpoint='write'
                body=$(printf '{"authorization_model_id":"%s","deletes":{"tuple_keys":[%s]}}' "$model_id" "$tuple") ;;
            *)
                endpoint=check
                body=$(printf '{"authorization_model_id":"%s","tuple_key":%s,"consistency":"HIGHER_CONSISTENCY"}' "$model_id" "$tuple") ;;
        esac
        response=/tmp/fga-memory/response-$cycle-$operation.http
        request_path=/stores/$store_id/$endpoint
        : >"$response"
        transport_exit=0
        timeout --kill-after=1s 5s /bin/bash "$0" http-request "$request_path" "$body" "$response" || transport_exit=$?
        response_bytes=$(wc -c <"$response")
        printf 'FGA_MEMORY|request|{"cycle":%s,"operation":"%s","user":"%s","object":"%s","relation":"ownership","method":"POST","path":"%s","request_body_b64":"%s","response_b64":"%s","transport_exit":%s,"budget_seconds":5,"response_bytes":%s,"guest_seconds":%s}\n' \
            "$cycle" "$operation" "$user" "$object" "$request_path" "$(printf '%s' "$body" | base64 -w0)" \
            "$(base64 -w0 "$response")" "$transport_exit" "$response_bytes" "$SECONDS"
        (( transport_exit == 0 )) || failure "$transport_exit" http-transport-or-framing-failed
        requests=$((requests + 1))
        kill -0 "$fga_pid" 2>/dev/null || failure 1 openfga-exited-during-probe
    done
done
stage after-cycles
timeout --kill-after=1s 5s /bin/bash "$0" process-snapshot after-cycles "$fga_pid" "$SECONDS"
printf 'FGA_MEMORY|done|{"cycles":2,"requests":%s,"guest_seconds":%s}\n' "$requests" "$SECONDS"
# Only the host's strict HTTP/JSON reader can mark this component diagnostic passed.
# Leave the VM alive for capture; never emit the application's acceptance markers.
idle
