import hashlib, json, os, time, urllib.request, urllib.error

base = os.getenv("FGA_URL", "http://openfga:8080")
def call(path, body):
    req = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=5) as response:
        return json.load(response)

with open("/app/fga/model.json", "rb") as f:
    model_bytes = f.read()
model_hash = hashlib.sha256(model_bytes).hexdigest()
if os.path.exists("/data/config.json"):
    with open("/data/config.json", encoding="utf-8") as f:
        current = json.load(f)
    if current.get("model_sha256") != model_hash:
        raise SystemExit("Model changed; explicit migration is required")
    for attempt in range(60):
        try:
            with urllib.request.urlopen(base + "/stores/" + current["store_id"] + "/authorization-models/" + current["model_id"], timeout=5) as response:
                if response.status == 200:
                    print("Using existing store and model")
                    raise SystemExit(0)
        except urllib.error.HTTPError as exc:
            if exc.code == 404:
                raise SystemExit("Existing store/model missing; refusing silent reset")
            time.sleep(1)
        except OSError:
            time.sleep(1)
    raise SystemExit("OpenFGA unavailable")

for attempt in range(60):
    try:
        store = call("/stores", {"name": "axum-lab"})["id"]
        break
    except (OSError, urllib.error.URLError):
        time.sleep(1)
else:
    raise SystemExit("OpenFGA unavailable")
model = call("/stores/" + store + "/authorization-models", json.loads(model_bytes))["authorization_model_id"]
with open("/app/fga/legacy.json") as f:
    tuples = json.load(f)
call("/stores/" + store + "/write", {"authorization_model_id": model, "writes": {"tuple_keys": tuples}})
with open("/data/config.json.tmp", "w") as f:
    json.dump({"store_id": store, "model_id": model, "model_sha256": model_hash}, f)
os.replace("/data/config.json.tmp", "/data/config.json")
print("Demo migration complete")
