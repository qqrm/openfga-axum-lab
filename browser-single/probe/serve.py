#!/usr/bin/env python3
"""Serve a supplied container2wasm artifact on loopback, without a backend."""
import argparse
import functools
import http.server
from pathlib import Path
from urllib.parse import unquote, urlsplit


class Handler(http.server.SimpleHTTPRequestHandler):
    artifact = None
    extensions_map = {**http.server.SimpleHTTPRequestHandler.extensions_map,
                      ".wasm": "application/wasm", ".js": "text/javascript"}

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        self.send_header("X-Content-Type-Options", "nosniff")
        super().end_headers()

    def translate_path(self, path):
        path = unquote(urlsplit(path).path)
        if path.startswith("/artifact/"):
            candidate = (self.artifact / path.removeprefix("/artifact/")).resolve()
            if not candidate.is_relative_to(self.artifact) or not candidate.is_file():
                return str(self.artifact / "__not_found__")
            return str(candidate)
        return super().translate_path(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact", type=Path, required=True)
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()
    Handler.artifact = args.artifact.resolve(strict=True)
    for filename in ("load.js", "arg-module.js", "out.js", "qemu-system-x86_64.data", "qemu-system-x86_64.wasm"):
        if not (Handler.artifact / filename).is_file():
            parser.error(f"Missing artifact file: {filename}")
    handler = functools.partial(Handler, directory=str(Path(__file__).resolve().parent))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), handler)
    print(f"http://127.0.0.1:{server.server_port}/", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
