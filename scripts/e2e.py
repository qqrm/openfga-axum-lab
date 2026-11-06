import json
import os
import time
import urllib.error
import urllib.request

base = os.getenv("APP_URL", "http://app:3000")

def req(method, path, user=None, body=None):
    headers = {} if user is None else {"Authorization": "Bearer " + user}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    request = urllib.request.Request(base + path, method=method, headers=headers, data=data)
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            payload = response.read()
            return response.status, json.loads(payload) if payload.startswith((b"[", b"{")) else payload
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read()

def expect(method, path, user, status, body=None):
    code, payload = req(method, path, user, body)
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

alice, bob, carol, dave = ("demo-alice", "demo-bob", "demo-carol", "demo-dave")
expect("GET", "/pets", None, 401)
expect("GET", "/pets", "invalid-token", 401)
assert {p["id"] for p in expect("GET", "/pets", alice, 200)} >= {"fox", "cat", "dog"}
assert {p["id"] for p in expect("GET", "/pets", carol, 200)} == {"fox"}
assert {p["id"] for p in expect("GET", "/pets", dave, 200)} == {"cat"}
expect("GET", "/pets/fox", dave, 403)
expect("GET", "/pets/fox", bob, 200)
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
created = expect("POST", "/pets", bob, 201, {"name":"Biscuit","species":"rabbit","biography":"Likes carrots."})
pet_path = "/pets/" + created["id"]
expect("GET", pet_path, bob, 200)
expect("GET", pet_path, carol, 403)
expect("PUT", pet_path + "/relations/viewer/carol", bob, 204)
expect("GET", pet_path, carol, 200)
expect("DELETE", pet_path + "/relations/viewer/carol", bob, 204)
expect("GET", pet_path, carol, 403)
expect("DELETE", pet_path, bob, 204)
expect("GET", pet_path, alice, 404)
expect("POST", "/pets", alice, 400, {"name":"","species":"fox","biography":""})
print("Pet API end-to-end scenarios passed")
