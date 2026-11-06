import json
import os
import time
import urllib.error
import urllib.request
from client import login

base = os.getenv("APP_URL", "http://app:3000")
for _ in range(60):
    try:
        token = login(base, "bob")
        break
    except OSError:
        time.sleep(1)
else:
    raise SystemExit("App did not restart")
req = urllib.request.Request(base + "/pets/dog", headers={"Authorization": "Bearer " + token})
try:
    urllib.request.urlopen(req, timeout=5)
except urllib.error.HTTPError as exc:
    assert exc.code == 404, exc.code
else:
    raise AssertionError("Deleting pet became visible")
with open("/data/config.json", encoding="utf-8") as file:
    config = json.load(file)
req = urllib.request.Request(
    "http://openfga:8080/stores/" + config["store_id"] + "/read",
    data=json.dumps({"tuple_key": {"object": "pet:dog"}}).encode(),
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(req, timeout=5) as response:
    assert not json.load(response).get("tuples", [])
print("Deletion recovery removed all pet tuples")
