"""Client helpers for isolated demonstration tests, no persistent credentials."""
import json
import os
import urllib.request

def login(base, username):
    body = json.dumps({"username": username, "password": os.getenv("DEMO_PASSWORD", "local-demo-password")}).encode()
    req = urllib.request.Request(base + "/session", data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as response:
        return json.load(response)["token"]
