"""Run with 'before' and 'after' around a full Compose stop/start."""
import json
import os
import sys
import time
import urllib.error
import urllib.request
from client import login

base = os.getenv("APP_URL", "http://127.0.0.1:3000")
tokens = {}
def call(method, path, user, body=None):
    if user not in tokens:
        tokens[user] = login(base, user)
    headers = {"Authorization": "Bearer " + tokens[user]}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    req = urllib.request.Request(base + path, method=method, headers=headers, data=data)
    try:
        with urllib.request.urlopen(req, timeout=5) as response:
            raw = response.read()
            return response.status, json.loads(raw) if raw.startswith(b"{") else raw
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read()

for _ in range(60):
    try:
        if call("GET", "/pets/fox", "carol")[0] == 200:
            break
    except OSError:
        pass
    time.sleep(1)
else:
    raise SystemExit("Pet API unavailable")

if sys.argv[1:] == ["before"]:
    status, _ = call("PATCH", "/pets/fox", "carol", {"name":"Mika","species":"fox","biography":"Persistent across restart"})
    assert status == 204, status
    status, _ = call("PUT", "/pets/fox/relations/viewer/dave", "carol")
    assert status == 204, status
elif sys.argv[1:] == ["after"]:
    status, body = call("GET", "/pets/fox", "dave")
    assert status == 200 and body["biography"] == "Persistent across restart", (status, body)
    status, _ = call("DELETE", "/pets/fox/relations/viewer/dave", "carol")
    assert status == 204, status
    status, _ = call("GET", "/pets/fox", "dave")
    assert status == 403, status
else:
    raise SystemExit("Expected before or after")
print("Persistence " + sys.argv[1] + " passed")
