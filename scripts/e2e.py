import json, os, urllib.error, urllib.request, time

base = os.getenv("APP_URL", "http://app:3000")
def req(method, path, user=None):
    request = urllib.request.Request(base + path, method=method, headers={} if user is None else {"x-demo-user": user})
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            body = response.read()
            return response.status, json.loads(body) if body.startswith(b"[") else body
    except urllib.error.HTTPError as e:
        return e.code, e.read()
def expect(method, path, user, status):
    code, body = req(method, path, user)
    assert code == status, (method, path, user, code, status, body)
    return body
for _ in range(60):
    try:
        if req("GET", "/healthz")[0] == 200:
            break
    except OSError:
        time.sleep(1)
else:
    raise SystemExit("App unavailable")
for u, p, status in [(None,"one",401),("alice","one",200),("bob","one",200),("bob","private",403),("carol","private",200),("dave","one",403),("dave","two",200),("alice","nonexistent",404),("user:alice","one",401)]:
    expect("GET", "/dashboards/" + p, u, status)
assert set(expect("GET", "/dashboards", "bob", 200)) == {"one", "two"}
assert set(expect("GET", "/dashboards", "carol", 200)) == {"private"}
expect("PUT", "/dashboards/one/relations/viewer/eve", "bob", 403)
expect("PUT", "/dashboards/one/relations/viewer/eve", "alice", 204)
expect("GET", "/dashboards/one", "eve", 200)
expect("DELETE", "/dashboards/one/relations/viewer/eve", "alice", 204)
expect("GET", "/dashboards/one", "eve", 403)
expect("PUT", "/dashboards/one/relations/editor/eve", "alice", 204)
expect("GET", "/dashboards/one", "eve", 200)
expect("DELETE", "/dashboards/one/relations/editor/eve", "alice", 204)
expect("PUT", "/dashboards/private/relations/owner/eve", "carol", 204)
expect("GET", "/dashboards/private", "eve", 200)
expect("DELETE", "/dashboards/private/relations/owner/eve", "carol", 204)
expect("PUT", "/dashboards/private/relations/parent/acme", "carol", 204)
expect("GET", "/dashboards/private", "bob", 200)
expect("DELETE", "/dashboards/private/relations/parent/acme", "carol", 204)
expect("GET", "/dashboards/private", "bob", 403)
expect("PUT", "/dashboards/one/relations/unknown/eve", "alice", 400)
expect("PUT", "/organizations/acme/members/eve", "bob", 403)
expect("PUT", "/organizations/acme/members/eve", "alice", 204)
expect("GET", "/dashboards/one", "eve", 200)
expect("DELETE", "/organizations/acme/members/eve", "alice", 204)
expect("GET", "/dashboards/one", "eve", 403)
print("End-to-end scenarios passed")
