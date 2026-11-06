import json
import concurrent.futures
import os
import time
import urllib.error
import urllib.request
import uuid
from client import login

base = os.getenv("APP_URL", "http://app:3000")

def req(method, path, user=None, body=None, key=None):
    headers = {} if user is None else {"Authorization": "Bearer " + user}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    if key is not None:
        headers["Idempotency-Key"] = key
    request = urllib.request.Request(base + path, method=method, headers=headers, data=data)
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            payload = response.read()
            return response.status, json.loads(payload) if payload.startswith((b"[", b"{")) else payload
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read()

def expect(method, path, user, status, body=None, key=None):
    code, payload = req(method, path, user, body, key)
    assert code == status, (method, path, user, code, status, payload)
    return payload

for _ in range(60):
    try:
        if req("GET", "/healthz")[0] == 200:
            break
    except OSError:
        time.sleep(1)
else:
    raise SystemExit("App unavailable")

expect("POST", "/session", None, 401, {"username":"alice","password":"wrong-password"})
expect("POST", "/session", None, 401, {"username":"nonexistent","password":"wrong-password"})
alice, bob, carol, dave = (login(base, name) for name in ("alice", "bob", "carol", "dave"))
expect("GET", "/pets", None, 401)
expect("GET", "/pets", "invalid-token", 401)
metric_code, metrics = req("GET", "/metrics")
assert metric_code == 200 and b"http_requests_total" in metrics
assert b'demo-' not in metrics and b'Authorization' not in metrics
assert {p["id"] for p in expect("GET", "/pets", alice, 200)} >= {"fox", "cat", "dog"}
assert {p["id"] for p in expect("GET", "/pets", carol, 200)} == {"fox"}
assert {p["id"] for p in expect("GET", "/pets", dave, 200)} == {"cat"}
expect("GET", "/pets/fox", dave, 403)
expect("GET", "/pets/fox", bob, 200)
_, metric_body = req("GET", "/metrics")
assert b'route="/pets/{id}"' in metric_body
assert b'route="/pets/fox"' not in metric_body
edit = {"name": "Mika", "species": "fox", "biography": "Mika likes apples."}
expect("PATCH", "/pets/fox", dave, 403, edit)
expect("PATCH", "/pets/fox", bob, 403, edit)
expect("PUT", "/pets/fox/relations/caregiver/dave", bob, 403)
expect("PUT", "/pets/fox/relations/caregiver/dave", carol, 204)
expect("PUT", "/pets/fox/relations/caregiver/dave", carol, 204)
expect("PATCH", "/pets/fox", dave, 204, edit)
assert expect("GET", "/pets/fox", carol, 200)["biography"] == edit["biography"]
expect("DELETE", "/pets/fox/relations/caregiver/dave", carol, 204)
expect("DELETE", "/pets/fox/relations/caregiver/dave", carol, 204)
expect("GET", "/pets/fox", dave, 403)
expect("POST", "/pets", dave, 403, edit)
payload = {"name":"Biscuit","species":"rabbit","biography":"Likes carrots."}
key = str(uuid.uuid4())
expect("POST", "/pets", bob, 400, payload)
with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
    results = list(pool.map(lambda _: req("POST", "/pets", bob, payload, key), range(6)))
assert all(code == 201 for code, _ in results), results
created = results[0][1]
assert all(body == created for _, body in results), results
assert expect("POST", "/pets", bob, 201, payload, key) == created
expect("POST", "/pets", bob, 409, {**payload, "name":"Different"}, key)
pet_path = "/pets/" + created["id"]
expect("GET", pet_path, bob, 200)
expect("GET", pet_path, carol, 403)
expect("PUT", pet_path + "/relations/viewer/carol", bob, 204)
expect("GET", pet_path, carol, 200)
expect("DELETE", pet_path + "/relations/viewer/carol", bob, 204)
expect("GET", pet_path, carol, 403)
expect("DELETE", pet_path, bob, 204)
expect("DELETE", pet_path, bob, 204)
expect("GET", pet_path, alice, 404)
expect("POST", "/pets", bob, 410, payload, key)
def assert_no_tuples(pet_id):
    with open("/data/config.json", encoding="utf-8") as file:
        config = json.load(file)
    read = urllib.request.Request(
        "http://openfga:8080/stores/" + config["store_id"] + "/read",
        data=json.dumps({"tuple_key":{"object":"pet:" + pet_id}}).encode(),
        headers={"Content-Type":"application/json"},
    )
    with urllib.request.urlopen(read, timeout=5) as response:
        assert not json.load(response).get("tuples", [])
assert_no_tuples(created["id"])
race = expect("POST", "/pets", bob, 201, payload, str(uuid.uuid4()))
race_path = "/pets/" + race["id"]
def concurrent_operation(index):
    if index == 0:
        return req("DELETE", race_path, bob)
    return req("PUT", race_path + "/relations/viewer/carol", bob)
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
    race_results = list(pool.map(concurrent_operation, range(8)))
assert race_results[0][0] == 204, race_results
assert all(code in (204, 404) for code, _ in race_results), race_results
assert_no_tuples(race["id"])
expect("POST", "/pets", alice, 400, {"name":"","species":"fox","biography":""}, str(uuid.uuid4()))
expect("DELETE", "/session", dave, 204)
expect("GET", "/pets", dave, 401)
print("Pet API end-to-end scenarios passed")
