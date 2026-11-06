"""Forward real OpenFGA requests; pause one matching response on demand."""
import json
import threading
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

lock = threading.Lock()
gate = None


class Gate:
    def __init__(self, match, fail):
        self.match = match
        self.fail = fail
        self.captured = threading.Event()
        self.release = threading.Event()
        self.claimed = False


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        # The proxy is test-only. Avoid recording request bodies or headers.
        pass

    def reply(self, status, body=b""):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def handle_request(self):
        global gate
        payload = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path == "/_gate/arm" and self.command == "POST":
            with lock:
                if gate is not None and not gate.release.is_set():
                    return self.reply(409)
                gate = Gate(**json.loads(payload))
            return self.reply(204)
        if self.path in ("/_gate/wait", "/_gate/release"):
            with lock:
                current = gate
            if current is None:
                return self.reply(404)
            if self.path.endswith("/wait"):
                return self.reply(204 if current.captured.wait(3) else 504)
            current.release.set()
            return self.reply(204)

        if self.path != "/healthz" and not self.path.startswith("/stores/"):
            return self.reply(404)
        request = urllib.request.Request(
            "http://openfga:8080" + self.path,
            data=payload if self.command == "POST" else None,
            method=self.command,
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                status, body = response.status, response.read()
        except urllib.error.HTTPError as error:
            status, body = error.code, error.read()
        except OSError:
            return self.reply(502)
        with lock:
            current = gate
            if (current is not None and not current.claimed
                    and self.path.endswith(current.match) and status == 200):
                current.claimed = True
                current.captured.set()
            else:
                current = None
        if current is not None:
            if not current.release.wait(10) or current.fail:
                return self.reply(502)
        return self.reply(status, body)

    do_GET = handle_request
    do_POST = handle_request


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
