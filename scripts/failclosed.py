import urllib.error
import urllib.request
import os
from client import login

base = os.getenv("APP_URL", "http://127.0.0.1:3000")
token = login(base, "alice")
for path in ("/pets/fox", "/pets"):
    request = urllib.request.Request(base + path, headers={"Authorization": "Bearer " + token})
    try:
        urllib.request.urlopen(request, timeout=5)
    except urllib.error.HTTPError as exc:
        assert exc.code == 503, (path, exc.code)
    else:
        raise AssertionError("OpenFGA outage did not close access: " + path)
print("OpenFGA outage failed closed")
