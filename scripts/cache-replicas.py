"""Exercise warm allows and revoke races against two real Axum replicas."""
import concurrent.futures
import json
import re
import sys
import time
import urllib.error
import urllib.request

from client import login

first, second = "http://app:3000", "http://app2:3000"
proxy = "http://authz-test-proxy:8080"
pet = "/pets/fox"
relation = pet + "/relations/caregiver/dave"


def request(base, method, path, token=None, body=None):
    headers = {"Content-Type": "application/json"}
    if token is not None:
        headers["Authorization"] = "Bearer " + token
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, method=method, headers=headers, data=data)
    try:
        with urllib.request.urlopen(req, timeout=8) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def expect(base, method, path, status, token=None, body=None):
    code, payload = request(base, method, path, token, body)
    assert code == status, (base, method, path, code, status)
    return payload


def metric(base, event):
    payload = expect(base, "GET", "/metrics", 200)
    pattern = rb'^authz_cache_events_total\{event="' + event.encode() + rb'"\} (\d+)$'
    match = re.search(pattern, payload, re.MULTILINE)
    return int(match[1]) if match else 0


def arm(match, fail=False):
    expect(proxy, "POST", "/_gate/arm", 204, body={"match": match, "fail": fail})


def wait_for_capture():
    expect(proxy, "GET", "/_gate/wait", 204)


def release():
    expect(proxy, "GET", "/_gate/release", 204)


def ready(base):
    for _ in range(60):
        try:
            if request(base, "GET", "/healthz")[0] == 200:
                return
        except OSError:
            pass
        time.sleep(1)
    raise AssertionError(f"{base} did not start")


for replica in (first, second):
    ready(replica)
dave = login(second, "dave")
if sys.argv[1:] == ["--after-restart"]:
    old = metric(second, "bypass_pending")
    expect(second, "GET", pet, 403, dave)
    assert metric(second, "bypass_pending") == old + 1
    print("The restarted replica still bypasses its cache after an uncertain write")
    sys.exit(0)

carol = login(first, "carol")

# Prove both independent caches served an actual positive hit.
expect(first, "PUT", relation, 204, carol)
for replica in (first, second):
    old = metric(replica, "hit_allow")
    expect(replica, "GET", pet, 200, dave)
    expect(replica, "GET", pet, 200, dave)
    assert metric(replica, "hit_allow") > old, f"{replica} had no positive hit"
expect(first, "DELETE", relation, 204, carol)
for replica in (first, second):
    expect(replica, "GET", pet, 403, dave)
print("A revoke invalidated warm positive decisions on both replicas")

# Capture a real OpenFGA allow, finish a revoke elsewhere, then deliver it.
expect(first, "PUT", relation, 204, carol)
old = metric(second, "retry")
with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
    arm("/check")
    future = pool.submit(request, second, "GET", pet, dave)
    try:
        wait_for_capture()
        expect(first, "DELETE", relation, 204, carol)
    finally:
        release()
    assert future.result()[0] == 403, "Old OpenFGA allow escaped the revision fence"
assert metric(second, "retry") > old
expect(second, "GET", pet, 403, dave)
print("A late OpenFGA allow was discarded and recomputed")

# Also race a genuine cache hit waiting for readiness against a revoke.
expect(first, "PUT", relation, 204, carol)
expect(second, "GET", pet, 200, dave)
old = metric(second, "retry")
with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
    arm("/healthz")
    future = pool.submit(request, second, "GET", pet, dave)
    try:
        wait_for_capture()
        expect(first, "DELETE", relation, 204, carol)
    finally:
        release()
    assert future.result()[0] == 403, "Old cache hit escaped the revision fence"
assert metric(second, "retry") > old
print("A concurrent cache hit was discarded after revocation")

# A backend readiness failure must not be hidden by a warm positive cache.
expect(first, "PUT", relation, 204, carol)
expect(second, "GET", pet, 200, dave)
with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
    arm("/healthz", fail=True)
    future = pool.submit(request, second, "GET", pet, dave)
    try:
        wait_for_capture()
    finally:
        release()
    assert future.result()[0] == 503, "Warm allow masked backend unavailability"

# The backend commits the deletion but the caller loses the response. Let
# reqwest's deadline expire while the proxy holds that real success response.
expect(first, "GET", pet, 200, dave)
expect(first, "GET", pet, 200, dave)
old = metric(first, "bypass_pending")
with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
    arm("/write")
    future = pool.submit(request, second, "DELETE", relation, carol)
    try:
        wait_for_capture()
        expect(first, "GET", pet, 403, dave)
        assert future.result()[0] == 503, "Lost mutation response was treated as success"
    finally:
        release()
assert metric(first, "bypass_pending") > old
expect(second, "GET", pet, 403, dave)

# A later successful write must not clear somebody else's ambiguous mutation.
old = metric(second, "bypass_pending")
expect(first, "PUT", relation, 204, carol)
expect(second, "GET", pet, 200, dave)
expect(first, "DELETE", relation, 204, carol)
expect(second, "GET", pet, 403, dave)
assert metric(second, "bypass_pending") >= old + 2
print("A lost write response left both replicas checking OpenFGA directly")
