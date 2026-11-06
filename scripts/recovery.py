import json
import os
import time
import urllib.error
import urllib.request
from client import login

base = os.getenv("APP_URL", "http://127.0.0.1:3000")
url = base + "/pets/recovery-fixture"
token = None
for _ in range(60):
    try:
        if token is None:
            token = login(base, "carol")
        request = urllib.request.Request(url, headers={"Authorization":"Bearer " + token})
        with urllib.request.urlopen(request, timeout=3) as response:
            body = json.load(response)
        assert body["id"] == "recovery-fixture"
        break
    except (OSError, urllib.error.HTTPError):
        time.sleep(1)
else:
    raise SystemExit("Pending pet did not recover")
request = urllib.request.Request(url, method="DELETE", headers={"Authorization":"Bearer " + token})
with urllib.request.urlopen(request, timeout=3) as response:
    assert response.status == 204
print("Pending creation recovered")
