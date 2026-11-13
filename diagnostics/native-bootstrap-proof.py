"""Native-only deadline proof: proxy real OpenFGA responses without changing them."""
import collections
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import threading
import time

ROOT = Path("artifacts/native-bootstrap")
STACK = "native-bootstrap-stack"
PROXY = "native-bootstrap-proxy"
SOURCE = "53ff85cfdd3d84c26ae432189790e1e0f72d61db"
PACKAGING = "d7f7ead003c774e30584798adfde38afb854528d"
MODEL_SHA = "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d"
FGA_SHA = "57ef124e5b339dcf1c3dbcbe2c3083c03f5db74a799a1472014e875292cc1066"


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def emit(event, **fields):
    print(json.dumps({"event": event, **fields}, separators=(",", ":")), flush=True)


def operation(method, path):
    if path == "/healthz":
        return "health"
    if method == "POST" and path == "/stores":
        return "create_store"
    if method == "POST" and re.fullmatch(r"/stores/[0-9A-HJKMNP-TV-Z]{26}/authorization-models", path):
        return "model_post"
    if method == "GET" and re.fullmatch(r"/stores/[0-9A-HJKMNP-TV-Z]{26}/authorization-models/[0-9A-HJKMNP-TV-Z]{26}", path):
        return "model_get"
    if method == "GET" and re.fullmatch(r"/stores/[0-9A-HJKMNP-TV-Z]{26}", path):
        return "get_store"
    if method == "POST" and re.fullmatch(r"/stores/[0-9A-HJKMNP-TV-Z]{26}/read", path):
        return "read"
    raise RuntimeError("Unexpected bootstrap proxy operation")


def serve_proxy():
    lock = threading.Lock()
    next_id = 0

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def forward(self):
            nonlocal next_id
            control = json.loads(Path("/native-proof/control.json").read_text())
            phase = control["phase"]
            op = operation(self.command, self.path)
            delay = control.get(op, 0) if op in ("model_post", "model_get") else 0
            require(delay in (0, 6), "Unexpected diagnostic delay")
            require(self.headers.get("Transfer-Encoding") is None, "Unexpected streamed request")
            length = int(self.headers.get("Content-Length", "0"))
            require(0 <= length <= 1024 * 1024, "Oversized bootstrap request")
            request_body = self.rfile.read(length)
            require(len(request_body) == length, "Incomplete bootstrap request")
            with lock:
                next_id += 1
                request_id = next_id
                emit("start", phase=phase, operation=op, request_id=request_id)
            upstream = http.client.HTTPConnection("127.0.0.1", 8080, timeout=10)
            try:
                headers = {key: value for key, value in self.headers.items()
                           if key.lower() not in ("host", "connection", "content-length")}
                upstream.request(self.command, self.path, body=request_body, headers=headers)
                response = upstream.getresponse()
                body = response.read(1024 * 1024 + 1)
                require(len(body) <= 1024 * 1024, "Oversized real OpenFGA response")
                status, response_headers = response.status, response.getheaders()
            finally:
                upstream.close()
            # Delay starts only after the real upstream response is complete.
            completed = time.monotonic()
            detail = {}
            if op == "read" and status == 200:
                decoded = json.loads(body)
                detail = {"tuple_count": len(decoded.get("tuples", [])),
                          "continuation_token": decoded.get("continuation_token", "")}
            with lock:
                emit("upstream", phase=phase, operation=op, request_id=request_id,
                     status=status, body_bytes=len(body),
                     body_sha256=hashlib.sha256(body).hexdigest(), delay_seconds=delay, **detail)
            time.sleep(delay)
            forwarded = False
            try:
                self.send_response_only(status)
                for key, value in response_headers:
                    if key.lower() not in ("connection", "transfer-encoding", "content-length"):
                        self.send_header(key, value)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)
                self.wfile.flush()
                forwarded = True
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                self.close_connection = True
                with lock:
                    emit("finished", phase=phase, operation=op, request_id=request_id,
                         status=status, forwarded=forwarded,
                         body_sha256=hashlib.sha256(body).hexdigest(),
                         delay_elapsed_ms=round((time.monotonic() - completed) * 1000))

        do_GET = forward
        do_POST = forward

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 18080), Handler)
    emit("proxy_ready", port=18080, upstream="127.0.0.1:8080")
    server.serve_forever()


def command(args, timeout=15, check=True):
    result = subprocess.run(args, text=True, capture_output=True, timeout=timeout)
    if check and result.returncode != 0:
        raise RuntimeError("Command failed: " + " ".join(args[:4]) + "\n" + result.stderr[-4000:])
    return result


def docker(*args, **kwargs):
    return command(["docker", *args], **kwargs)


def logs():
    result = docker("logs", PROXY, check=False)
    return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]


def wait_for(predicate, seconds, description):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if predicate():
            return
        state = docker("inspect", "-f", "{{.State.Running}}", STACK, check=False)
        require(state.returncode == 0 and state.stdout.strip() == "true", "Real stack exited: " + description)
        time.sleep(0.2)
    raise RuntimeError("Timed out: " + description)


def exists(path):
    return docker("exec", STACK, "test", "-f", path, check=False).returncode == 0


def phase(name, post_delay=0, get_delay=0):
    temporary = ROOT / "control.tmp"
    temporary.write_text(json.dumps({"phase": name, "model_post": post_delay, "model_get": get_delay}))
    temporary.replace(ROOT / "control.json")


def phase_events(name, event):
    return [entry for entry in logs() if entry.get("phase") == name and entry.get("event") == event]


def verify_phase(name, expected, delayed):
    started = phase_events(name, "start")
    require(collections.Counter(entry["operation"] for entry in started) == collections.Counter(expected),
            "Unexpected request count or retry in " + name)
    upstream = phase_events(name, "upstream")
    require(len(upstream) == len(started), "A request did not complete against real OpenFGA")
    for entry in upstream:
        expected_status = 201 if entry["operation"] in ("create_store", "model_post") else 200
        require(entry["status"] == expected_status, "Real OpenFGA did not confirm " + entry["operation"])
    for op in delayed:
        finished = [entry for entry in phase_events(name, "finished") if entry["operation"] == op]
        require(len(finished) == 1 and finished[0]["delay_elapsed_ms"] >= 5900,
                "The genuine response was not delayed more than five seconds")


def main():
    ROOT.mkdir(parents=True, exist_ok=True)
    image = (ROOT / "image-id.txt").read_text().strip()
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", image), "Invalid immutable image ID")
    image_info = json.loads(docker("image", "inspect", image).stdout)[0]
    require(image_info["Architecture"] == "amd64" and image_info["Os"] == "linux", "Unexpected image target")
    proof_dir = ROOT.resolve()
    shutil.copyfile("packaging/browser-single/entrypoint.sh", ROOT / "entrypoint.sh")
    gate = ROOT / "bootstrap-gate.sh"
    gate.write_text("""#!/usr/bin/env bash
set -euo pipefail
test "$#" -eq 1 && test "$1" = bootstrap
touch /tmp/native-bootstrap-ready
timeout 300s bash -c 'until test -f /tmp/native-start-bootstrap; do sleep 0.2; done'
FGA_URL=http://127.0.0.1:18080 BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS=30 /usr/local/bin/lab-tool bootstrap
touch /tmp/native-bootstrap-done
timeout 300s bash -c 'until test -f /tmp/native-start-app; do sleep 0.2; done'
""")
    gate.chmod(0o555)
    phase("held")
    owned = []
    try:
        for name in (STACK, PROXY):
            require(docker("inspect", name, check=False).returncode != 0, "Container name already exists")
        docker("run", "-d", "--name", STACK, "--label", "io.authzlab.native-bootstrap-run=" + os.environ["GITHUB_RUN_ID"],
               "--shm-size=128m", "--entrypoint", "/bin/bash",
               "-e", "BROWSER_LAB_EPHEMERAL=yes", "-e", "OPENFGA_LOG_LEVEL=warn",
               "-e", "OPENFGA_METRICS_ENABLED=false",
               "-e", "BROWSER_LAB_OPENFGA_BINARY=/native-openfga/openfga",
               "-e", "BROWSER_LAB_BOOTSTRAP_BINARY=/native-proof/bootstrap-gate.sh",
               "--mount", "type=bind,src=" + str(proof_dir) + ",dst=/native-proof,readonly",
               "--mount", "type=bind,src=" + str(Path("artifacts/native-bootstrap-input").resolve()) + ",dst=/native-openfga,readonly",
               image, "/native-proof/entrypoint.sh")
        owned.append(STACK)
        binary_hash = docker("exec", STACK, "sha256sum", "/usr/local/bin/lab-tool").stdout.split()[0]
        require(docker("exec", STACK, "sha256sum", "/native-openfga/openfga").stdout.split()[0] == FGA_SHA,
                "Actual mounted OpenFGA binary changed")
        wait_for(lambda: exists("/tmp/native-bootstrap-ready"), 240, "real PostgreSQL/OpenFGA startup")
        docker("run", "-d", "--name", PROXY, "--label", "io.authzlab.native-bootstrap-run=" + os.environ["GITHUB_RUN_ID"],
               "--network", "container:" + STACK, "--read-only", "--cap-drop", "ALL",
               "--security-opt", "no-new-privileges", "--user", "65534:65534",
               "--mount", "type=bind,src=" + str(proof_dir) + ",dst=/native-proof,readonly",
               "python:3.12-slim-bookworm", "python", "-B", "-u", "/native-proof/native-proof.py", "proxy",
               timeout=120)
        owned.append(PROXY)
        wait_for(lambda: any(entry.get("event") == "proxy_ready" for entry in logs()), 20, "real-response proxy")
        defaults = []
        for name, post_delay, get_delay, expected, delayed in [
            ("default-post", 6, 0, ["health", "create_store", "model_post"], ["model_post"]),
            ("default-get", 0, 6, ["health", "create_store", "model_post", "get_store", "model_get"], ["model_get"]),
        ]:
            phase(name, post_delay, get_delay)
            config = "/tmp/" + name + ".json"
            start = time.monotonic()
            result = docker("exec", STACK, "env", "-u", "BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS",
                            "FGA_URL=http://127.0.0.1:18080", "FGA_CONFIG=" + config,
                            "/usr/local/bin/lab-tool", "bootstrap", timeout=15, check=False)
            elapsed = time.monotonic() - start
            require(result.returncode != 0 and 4.5 <= elapsed < 15, "Default five-second operation did not fail as expected")
            require("Elapsed" in result.stderr or "TimedOut" in result.stderr or "timed out" in result.stderr.lower(),
                    "Default failure was not an observed request deadline")
            require(not exists(config), "Failed bootstrap published a configuration")
            wait_for(lambda: all(any(entry["operation"] == op for entry in phase_events(name, "finished")) for op in delayed),
                     10, "completion of delayed real response")
            verify_phase(name, expected, delayed)
            record = {"phase": name, "exit": result.returncode, "elapsed_ms": round(elapsed * 1000),
                      "config_published": False, "stderr": result.stderr[-4000:]}
            defaults.append(record)
            emit("default_fail_closed", **record)
        phase("opt-in", 6, 6)
        docker("exec", STACK, "touch", "/tmp/native-start-bootstrap")
        wait_for(lambda: exists("/tmp/native-bootstrap-done"), 90, "opt-in real model provisioning")
        wait_for(lambda: all(any(entry["operation"] == op for entry in phase_events("opt-in", "finished"))
                             for op in ("model_post", "model_get")), 10, "completed opt-in delayed responses")
        verify_phase("opt-in", ["health", "create_store", "model_post", "get_store", "model_get", "read"],
                     ["model_post", "model_get"])
        read = [entry for entry in phase_events("opt-in", "upstream") if entry["operation"] == "read"]
        require(len(read) == 1 and read[0]["tuple_count"] == 0 and read[0]["continuation_token"] == "",
                "The real new store was not proven empty")
        config = json.loads(docker("exec", STACK, "cat", "/tmp/browser-lab/config.json").stdout)
        require(config["model_sha256"] == MODEL_SHA and all(
                re.fullmatch(r"[0-9A-HJKMNP-TV-Z]{26}", config[key]) for key in ("store_id", "model_id")),
                "Published binding identity differs")
        models = docker("exec", STACK, "psql", "-X", "-qAt", "-w", "-v", "ON_ERROR_STOP=1",
                        "-h", "/tmp/browser-lab", "-U", "authzlab", "-d", "openfga",
                        "-c", "SELECT count(*) FROM authorization_model").stdout.strip()
        require(int(models) == 3, "Expected the three actual PostgreSQL authorization models")
        phase("passthrough")
        docker("exec", STACK, "touch", "/tmp/native-start-app")
        wait_for(lambda: docker("exec", STACK, "/usr/local/bin/lab-tool", "probe", check=False).returncode == 0,
                 240, "real Axum readiness and migration")
        passes = []
        for number in (1, 2):
            result = docker("exec", "-e", "SMOKE_ALLOW_MUTATIONS=true", STACK,
                            "/usr/local/bin/lab-tool", "acceptance", timeout=60)
            accepted = json.loads(result.stdout)
            require(accepted.get("accepted") is True and accepted.get("statuses") == [403, 204, 200, 204, 403],
                    "Original real native acceptance did not pass")
            (ROOT / ("acceptance-" + str(number) + ".json")).write_text(result.stdout)
            passes.append(accepted)
        result = {"passed": True, "scope": "fresh native candidate image; not browser acceptance",
                  "source_commit": SOURCE, "packaging_commit": PACKAGING, "image_id": image,
                  "binary_sha256": binary_hash, "openfga_binary_sha256": FGA_SHA,
                  "default_failures": defaults, "opt_in_model_timeout_seconds": 30,
                  "delayed_model_post_and_get_seconds": 6, "actual_postgres_model_count": int(models),
                  "native_passes": passes, "browser_acceptance_verified": False}
        (ROOT / "native-result.json").write_text(json.dumps(result, indent=2) + "\n")
        emit("native_proof", **result)
    finally:
        for name in owned:
            result = docker("logs", name, check=False)
            (ROOT / (name + ".log")).write_text(result.stdout + result.stderr)
        if STACK in owned:
            for name in ("initdb.log", "postgres.log"):
                docker("cp", STACK + ":/tmp/browser-lab/" + name, str(ROOT / name), check=False)
        for name in reversed(owned):
            docker("rm", "-f", name, check=False)


if __name__ == "__main__":
    if sys.argv[1:] == ["proxy"]:
        serve_proxy()
    elif not sys.argv[1:]:
        main()
    else:
        raise SystemExit("Unexpected native proof mode")
