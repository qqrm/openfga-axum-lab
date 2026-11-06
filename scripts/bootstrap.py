import json, os, time, urllib.request, urllib.error

base = os.getenv("FGA_URL", "http://openfga:8080")
def call(path, body):
    req = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=5) as response:
        return json.load(response)

for attempt in range(60):
    try:
        store = call("/stores", {"name": "axum-lab"})["id"]
        break
    except (OSError, urllib.error.URLError):
        time.sleep(1)
else:
    raise SystemExit("OpenFGA unavailable")
with open("/app/fga/model.json") as f:
    model = call("/stores/" + store + "/authorization-models", json.load(f))["authorization_model_id"]
with open("/app/fga/legacy.json") as f:
    tuples = json.load(f)
call("/stores/" + store + "/write", {"authorization_model_id": model, "writes": {"tuple_keys": tuples}})
with open("/data/config.json.tmp", "w") as f:
    json.dump({"store_id": store, "model_id": model}, f)
os.replace("/data/config.json.tmp", "/data/config.json")
print("Demo migration complete")
