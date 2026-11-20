#!/usr/bin/env python3
"""Restore pinned browser inputs for an allowlisted diagnostic; no VM/build.

All original producer identities and inner ZIP hashes remain immutable.
An expired or removed source artifact may be restored from one verified mirror.
"""
import hashlib, json, os, pathlib, re, stat, time, urllib.error, urllib.request, zipfile
from urllib.parse import urlsplit
from datetime import datetime, timezone
M = 1024**2
REPO = "qqrm/openfga-axum-lab"
root = pathlib.Path("prepared")
root.mkdir(exist_ok=False)
source_root = pathlib.Path("source-candidate")
# Fill both artifacts only from one completed successful combined native run.
# Missing pins stop before artifact reads, browser installation or VM startup.
native = {
  "run": 43500949883,
  "head": "bb8284f17e78667c6ea46a2de02de5dc3f3507af",
  "job": 132146260409,
  "bootstrap": {
    "artifact": 14074756932,
    "name": "browser-bootstrap-openfga10-paths",
    "bytes": 3321954,
    "sha256": "584367cfa6d93ab98593ac9cc939695030b23dcc5634d5da762bee8455926917"
  },
  "evidence": {
    "artifact": 14075336387,
    "name": "browser-openfga10-paths-native-evidence",
    "bytes": 24005932,
    "sha256": "7049ffbd41ba8a33f7f300fcf6c65ab3741145ba08a087b0c0e607b615203526"
  },
  "binaries": {
    "openfga": {
      "bytes": 63074488,
      "sha256": "d779834ae225c95502b957efc96a6153937bb2f5f2a1cef9f134847ae8c0d02b"
    },
    "bootstrap": {
      "bytes": 7444640,
      "sha256": "e1143b7d5f8cc564e09d12c8f685bd7cf33bd0beb1505f2b7cfc1f6f53e1d275"
    }
  }
}
if (type(native["run"]) is not int or native["run"] <= 0
    or not isinstance(native["head"], str) or re.fullmatch(r"[0-9a-f]{40}", native["head"]) is None
    or any(type(item["artifact"]) is not int or item["artifact"] <= 0
           or type(item["bytes"]) is not int or not 0 < item["bytes"] <= 192*M
           or not isinstance(item["sha256"], str) or re.fullmatch(r"[0-9a-f]{64}", item["sha256"]) is None
           for item in [native["bootstrap"], native["evidence"]])):
    raise SystemExit("Combined native OpenFGA/bootstrap artifact pins are not verified yet")
if type(native["job"]) is not int or native["job"] <= 0:
    raise SystemExit("The exact completed native job pin is missing")
for role in ["openfga", "bootstrap"]:
    record = native["binaries"][role]
    if (not isinstance(record, dict) or type(record.get("bytes")) is not int
        or not 0 < record["bytes"] <= 128*M or not isinstance(record.get("sha256"), str)
        or re.fullmatch(r"[0-9a-f]{64}", record["sha256"]) is None):
        raise SystemExit("The actual successfully verified native binary pin is missing: " + role)
if native["bootstrap"]["artifact"] == native["evidence"]["artifact"]:
    raise SystemExit("The two named native artifacts must be distinct")
# Immutable successful transport-only proof; unresolved pins fail before downloads.
lightweight = {
  "run": 43470193142,
  "head": "0a8d7dc4ba611fe7071b34005410f4dc832d7b71",
  "artifact": 14060071538,
  "name": "browser-log-transport-candidate",
  "bytes": 46909,
  "sha256": "2845492a3a08a09f4fc957b7907b920362cf521d96545e5569afc9918182cb34"
}
if (type(lightweight["run"]) is not int or lightweight["run"] <= 0
    or type(lightweight["artifact"]) is not int or lightweight["artifact"] <= 0
    or type(lightweight["bytes"]) is not int or not 0 < lightweight["bytes"] <= 16*M
    or not isinstance(lightweight["sha256"], str)
    or re.fullmatch(r"[0-9a-f]{64}", lightweight["sha256"]) is None):
    raise SystemExit("Successful exact-source lightweight transport artifact pins are not verified yet")
specs = [
  ("lightweight", lightweight["run"], lightweight["head"], lightweight["artifact"], lightweight["name"],
   lightweight["bytes"], lightweight["sha256"], "success", "push"),
  ("retained", 43402635950, "379e3280c2a1c789071292af141648eb2fc3ffbc", 14029511154,
   "pages-browser-vm-evidence", 260830150, "73e4c4d2f2204ef4da0450e472b50ffb507e52b6a46fe55e0cab153b4fa3976e", "failure", "pull_request"),
  ("kernel", 43473084506, "84af5832aed519e7f0628e36afd8d2fc3e4cc270", 14061334425,
   "browser-virtio-rng-kernel-candidate", 3689588, "c71c1e7fdb705e1b5351ed82d119322c0ff7ff9f9f068d5a75a611aded2935ad", "success", "push"),
  ("native", native["run"], native["head"], native["evidence"]["artifact"], native["evidence"]["name"],
   native["evidence"]["bytes"], native["evidence"]["sha256"], "success", "push"),
  ("bootstrap", native["run"], native["head"], native["bootstrap"]["artifact"], native["bootstrap"]["name"],
   native["bootstrap"]["bytes"], native["bootstrap"]["sha256"], "success", "push")
]
def require(ok, message):
    if not ok: raise SystemExit(message)
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs): return None
opener = urllib.request.build_opener(NoRedirect())
def request(suffix):
    return urllib.request.Request("https://api.github.com/repos/" + REPO + suffix, headers={
      "Authorization": "Bearer " + os.environ["ARTIFACT_READ_TOKEN"],
      "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28",
      "User-Agent": "pr36-openfga-memory-component"})
def api(suffix, deadline=None):
    timeout = 20
    if deadline is not None:
        remaining = deadline - time.monotonic()
        require(remaining > 0, "Current job metadata observation budget expired")
        timeout = min(timeout, remaining)
    with opener.open(request(suffix), timeout=timeout) as response: raw = response.read(M + 1)
    if deadline is not None:
        require(time.monotonic() <= deadline, "Current job metadata observation budget expired")
    require(len(raw) <= M, "Metadata response exceeded budget")
    return json.loads(raw)
def check_members(package):
    names = package.namelist()
    require(len(names) <= 10000 and len(names) == len(set(names)), "Invalid ZIP member count/duplicates")
    require(all(not n.startswith("/") and "\\" not in n and "\0" not in n
                and ".." not in n.split("/") for n in names), "Unsafe ZIP member name")
def extract(package, name, destination, limit, allow_empty=False):
    info = package.getinfo(name)
    mode = info.external_attr >> 16
    require(not info.is_dir() and not stat.S_ISLNK(mode) and
            stat.S_IFMT(mode) in (0, stat.S_IFREG) and (0 if allow_empty else 1) <= info.file_size <= limit,
            "Invalid selected ZIP member: " + name)
    destination.parent.mkdir(parents=True, exist_ok=True)
    size, digest = 0, hashlib.sha256()
    with package.open(info) as source, destination.open("xb") as output:
        while block := source.read(M):
            size += len(block)
            require(size <= info.file_size and size <= limit, "Member read exceeded budget")
            output.write(block); digest.update(block)
    require(size == info.file_size, "Truncated selected ZIP member")
    return {"bytes": size, "sha256": digest.hexdigest()}
# The mirror retains original ZIP bytes and producer proofs. It is a recovery
# source only: no original producer is relabeled as this diagnostic run.
MIRROR = {
    "run": 44663922895,
    "head": "d4794e07c8e9cefef6271f498c6c309ce52b661c",
    "job": 136048513540,
    "artifact": 14597878044,
    "name": "pr36-vcpu-reproduction-inputs",
    "bytes": 308249993,
    "sha256": "9d21628183e1d211558959446bbb71d76ef3fcbe21d221cc8f9c4fc51edbaa20",
}
mirror_proof = None

def artifact_metadata(aid):
    try:
        return api("/actions/artifacts/" + str(aid)), None
    except urllib.error.HTTPError as error:
        status = error.code
        error.close()
        require(status in (404, 410), "Artifact metadata failed outside the permitted missing/expired case")
        return None, "original_artifact_metadata_http_" + str(status)

def verify_artifact(metadata, aid, name, run_id, head, size, sha):
    require(metadata["id"] == aid and metadata["name"] == name
            and metadata["workflow_run"]["id"] == run_id
            and metadata["workflow_run"]["head_sha"] == head
            and metadata["size_in_bytes"] == size
            and metadata["digest"] == "sha256:" + sha,
            "Pinned artifact identity mismatch")
    require(type(metadata["expired"]) is bool, "Malformed original artifact expiry flag")

def download_archive(aid, destination, size, sha):
    try:
        opener.open(request("/actions/artifacts/" + str(aid) + "/zip"), timeout=20)
        raise SystemExit("Expected artifact redirect")
    except urllib.error.HTTPError as error:
        require(error.code == 302, "Artifact redirect was not HTTP 302")
        location = error.headers.get("Location", "")
        error.close()
    target = urlsplit(location)
    host = target.hostname or ""
    require(target.scheme == "https" and target.port in (None, 443)
            and target.username is None and target.password is None
            and (host.endswith(".blob.core.windows.net") or host.endswith(".actions.githubusercontent.com")),
            "Unexpected artifact storage host")
    count, digest = 0, hashlib.sha256()
    deadline = time.monotonic() + 180
    try:
        # The bearer token is never sent to the signed storage endpoint.
        with opener.open(urllib.request.Request(location), timeout=30) as response, destination.open("xb") as output:
            while block := response.read(M):
                require(time.monotonic() <= deadline, "Artifact download exceeded its finite deadline")
                count += len(block)
                require(count <= size, "Archive exceeded pinned size")
                digest.update(block)
                output.write(block)
    except (urllib.error.URLError, TimeoutError):
        raise SystemExit("Artifact storage network failure") from None
    require(count == size and digest.hexdigest() == sha, "Artifact ZIP SHA/size mismatch")
    return {"bytes": count, "sha256": sha}

def retained_mirror():
    global mirror_proof
    archive = root / "reproduction-inputs.zip"
    if mirror_proof is not None:
        return archive, mirror_proof
    run = api("/actions/runs/" + str(MIRROR["run"]))
    require(run["id"] == MIRROR["run"] and run["head_sha"] == MIRROR["head"]
            and run["head_branch"] == "diag/pr36-vcpu-20261120"
            and run["path"] == ".github/workflows/pr36-vcpu-gate.yml"
            and run["status"] == "completed" and run["conclusion"] == "failure"
            and run["run_attempt"] == 1 and run["event"] == "push",
            "Retained-input source run identity mismatch")
    artifact = api("/actions/artifacts/" + str(MIRROR["artifact"]))
    verify_artifact(artifact, MIRROR["artifact"], MIRROR["name"], MIRROR["run"],
                    MIRROR["head"], MIRROR["bytes"], MIRROR["sha256"])
    require(artifact["expired"] is False, "Retained-input mirror expired")
    job = api("/actions/jobs/" + str(MIRROR["job"]))
    require(job["id"] == MIRROR["job"] and job["run_id"] == MIRROR["run"]
            and job["head_sha"] == MIRROR["head"]
            and job["name"] == "vCPU browser acceptance (chromium, 1 CPU)"
            and job["status"] == "completed" and job["conclusion"] == "failure",
            "Retained-input job identity mismatch")
    upload = [step for step in job["steps"]
              if step["name"] == "Retain original verified archives before the source artifacts expire"]
    require(len(upload) == 1 and upload[0]["status"] == "completed"
            and upload[0]["conclusion"] == "success", "Retained-input upload step did not succeed")
    verified = download_archive(MIRROR["artifact"], archive, MIRROR["bytes"], MIRROR["sha256"])
    with zipfile.ZipFile(archive) as package:
        check_members(package)
        require(len(package.infolist()) <= 64, "Retained-input mirror member count exceeded budget")
    mirror_proof = {**MIRROR, "archive": verified, "uploadJob": job, "uploadStep": upload[0]}
    (root / "retained-input-mirror-provenance.json").write_text(json.dumps(mirror_proof, indent=2) + "\n")
    return archive, mirror_proof

def unique_retained_member(package, basename):
    matches = [item.filename for item in package.infolist()
               if not item.is_dir() and pathlib.PurePosixPath(item.filename).name == basename]
    require(len(matches) == 1, "Expected exactly one retained member: " + basename)
    return matches[0]

def obtain_archive(label, run_id, head, aid, name, size, sha):
    metadata, recovery_reason = artifact_metadata(aid)
    destination = root / (label + ".zip")
    if metadata is not None:
        verify_artifact(metadata, aid, name, run_id, head, size, sha)
        if metadata["expired"] is False:
            verified = download_archive(aid, destination, size, sha)
            return destination, verified["bytes"], None
        recovery_reason = "original_artifact_expired"
    archive, retained = retained_mirror()
    with zipfile.ZipFile(archive) as package:
        member = unique_retained_member(package, label + ".zip")
        actual = extract(package, member, destination, size)
        require(actual == {"bytes": size, "sha256": sha}, "Retained inner ZIP differs from original pin")
        proof_member = unique_retained_member(package, label + "-archive-provenance.json")
        proof_destination = root / "retained-producer-provenance" / (label + ".json")
        proof_record = extract(package, proof_member, proof_destination, 4*M)
        original = json.loads(proof_destination.read_text())
        require(original["artifact"] == aid and original["run"] == run_id
                and original["archive"] == {"bytes": size, "sha256": sha},
                "Retained original producer provenance differs")
        producer_head = original.get("head", original.get("workflowCommit"))
        require(producer_head == head, "Retained original producer commit differs")
        if label == "candidate-emulator":
            require(original["attempt"] == 1 and original["buildJob"]["id"] == 133658307719,
                    "Retained emulator producer identity differs")
        else:
            require(original["label"] == label and original["artifact_name"] == name,
                    "Retained original artifact role differs")
    recovery = {"kind": "verified_original_zip_from_retained_mirror", "reason": recovery_reason,
                "originalArtifact": aid, "originalArtifactMetadata": metadata,
                "mirrorArtifact": MIRROR["artifact"], "mirrorRun": MIRROR["run"],
                "mirrorArchive": retained["archive"], "member": member,
                "originalProducerProvenanceMember": proof_member,
                "originalProducerProvenance": proof_record}
    return destination, actual["bytes"], recovery

engine = os.environ["BROWSER_DIAGNOSTIC_ENGINE"]
require(engine == "chromium", "This finite browser diagnostic runs only Chromium")
job_name = os.environ.get("BROWSER_DIAGNOSTIC_JOB_NAME", "OpenFGA memory browser component")
diagnostic_jobs = {
    "OpenFGA memory browser component": (
        "diag/pr36-vcpu-20261120", ".github/workflows/pr36-memory-component.yml"),
    "Browser full-stack datastore comparison": (
        "diag/pr36-stack-20261120", ".github/workflows/pr36-stack-continuation.yml"),
}
require(job_name in diagnostic_jobs, "Unexpected diagnostic job name")
expected_branch, expected_workflow = diagnostic_jobs[job_name]
current_run = int(os.environ["GITHUB_RUN_ID"])
current_attempt = int(os.environ["GITHUB_RUN_ATTEMPT"])
require(current_run > 0 and current_attempt == 1, "This finite diagnostic forbids workflow reruns")
observation_started = time.monotonic()
observation_deadline = observation_started + 30
observations = root / "job-start-observations.jsonl"
def observe(record):
    record["elapsed_ms"] = round((time.monotonic() - observation_started) * 1000)
    record["observed_at"] = datetime.now(timezone.utc).isoformat()
    encoded = json.dumps(record, ensure_ascii=True)
    with observations.open("a", encoding="utf-8") as output: output.write(encoded + "\n")
    print(encoded, flush=True)
current_info = api("/actions/runs/" + str(current_run), observation_deadline)
observe({"kind": "current_run", **{key: current_info.get(key) for key in [
    "id", "head_sha", "head_branch", "run_attempt", "event", "status"]}})
require(current_info["id"] == current_run and current_info["head_sha"] == os.environ["GITHUB_SHA"]
        and current_info["head_branch"] == expected_branch and current_info["path"] == expected_workflow
        and current_info["run_attempt"] == current_attempt and current_info["event"] == "push",
        "Current run immutable identity mismatch")
cpus = os.environ["BROWSER_DIAGNOSTIC_VCPUS"]
require(cpus == "2", "This finite browser diagnostic uses exactly two guest CPUs")
expected_job_id = None
for observation in range(1, 7):
    jobs = api("/actions/runs/" + str(current_run) + "/attempts/" + str(current_attempt) + "/jobs?per_page=100", observation_deadline)
    require(jobs["total_count"] <= 100, "Current run job list exceeded its bounded page")
    matching = [job for job in jobs["jobs"] if job["name"] == job_name]
    require(len(matching) == 1, "Expected exactly one current diagnostic job")
    job = matching[0]
    observe({"kind": "current_job", "observation": observation,
        **{key: job.get(key) for key in ["id", "name", "run_id", "head_sha", "status", "started_at"]},
        "reported_job_run_attempt": job.get("run_attempt")})
    require(type(job["id"]) is int and job["id"] > 0 and job["run_id"] == current_run
            and ("run_attempt" not in job or job["run_attempt"] == current_attempt)
            and job["head_sha"] == current_info["head_sha"],
            "Current job immutable identity mismatch")
    if expected_job_id is None: expected_job_id = job["id"]
    require(job["id"] == expected_job_id, "Current job identity changed between observations")
    require(job["status"] in ["queued", "in_progress"], "Current job is no longer eligible to start")
    started_at = job.get("started_at")
    require(started_at is None or (isinstance(started_at, str) and started_at.endswith("Z")),
            "Current job start metadata is malformed")
    if job["status"] == "in_progress" and started_at is not None:
        break
    require(observation < 6 and time.monotonic() + 2 < observation_deadline,
            "Current job start metadata did not become available within its observation budget")
    time.sleep(2)
require(time.monotonic() <= observation_deadline, "Current job metadata observation budget expired")
job_timing = {key: job[key] for key in ["id", "name", "run_id", "head_sha", "started_at"]}
job_timing["run_attempt"] = current_info["run_attempt"]
job_timing["source"] = "GitHub Actions current job API"
job_timing["observed_at"] = datetime.now(timezone.utc).isoformat()
(root / "job-timing.json").write_text(json.dumps(job_timing, indent=2) + "\n")
print(json.dumps({"current_job_timing": job_timing}), flush=True)
for label, run_id, head, aid, name, size, sha, conclusion, event in specs:
    run = api("/actions/runs/" + str(run_id))
    require(run["id"] == run_id and run["status"] == "completed" and run["conclusion"] == conclusion and
            run["head_sha"] == head and run["run_attempt"] == 1 and run["event"] == event,
            "Pinned source run identity mismatch")
    native_job = None
    if label == "native":
        native_job = api("/actions/jobs/" + str(native["job"]))
        require(native_job["id"] == native["job"] and native_job["run_id"] == run_id
                and native_job["head_sha"] == head
                and native_job["name"] == "native" and native_job["status"] == "completed"
                and native_job["conclusion"] == "success", "The fresh native job did not succeed")
        for expected_step in [
            "Check source identity, repository contract and both producer scripts",
            "Build and regression-test the fresh OpenFGA .10 producer",
            "Bind the fresh binary to exact inputs and actual regular and race tests",
            "Execute the versioned native bootstrap producer with this exact OpenFGA output",
            "Verify all four outputs and their real native OpenFGA provenance"
        ]:
            completed = [step for step in native_job["steps"] if step["name"] == expected_step]
            require(len(completed) == 1 and completed[0]["status"] == "completed"
                    and completed[0]["conclusion"] == "success", "Missing successful native proof step")
    archive_path, count, recovery = obtain_archive(label, run_id, head, aid, name, size, sha)
    members = {}
    selected_total = 0
    with zipfile.ZipFile(archive_path) as package:
        check_members(package)
        if label == "lightweight":
            prefix = "artifacts/browser-log-transport-candidate/"
            scenarios = ["synchronous_uart_burst", "separate_tasks_ack_gate", "separate_tasks_unblocked",
                         "finish_partial_utf16", "render_error_at_cutoff", "delivery_error", "pending_chunk_bound"]
            source_paths = ["browser-single/pages/log-stream.mjs", "browser-single/pages/launcher.js",
                            "browser-single/log-stream.test.mjs"]
            proof_paths = [prefix + "summary.json", prefix + "node-tests.log",
                           "diagnostics/check-log-transport-candidate.mjs"] + [
                           "source-candidate/" + path for path in source_paths]
            expected_files = set(proof_paths) | {
                prefix + browser + "-" + scenario + suffix
                for browser in ["chromium", "firefox"] for scenario in scenarios
                for suffix in [".json", "-console.txt", "-stream.jsonl"]}
            require({item.filename for item in package.infolist() if not item.is_dir()} == expected_files,
                    "Unexpected lightweight proof artifact members")
            selected = [(name, root / "lightweight-evidence" / name, 2*M) for name in proof_paths]
        elif label == "retained":
            selected = [
              ("browser-output/wasm/runtime-source.zip", root / "runtime-source.zip", 384*M),
              ("browser-output/wasm/runtime/derived-runtime.json", root / "retained/derived-runtime.json", 4*M),
              ("browser-output/wasm/runtime/startup-manifest.json", root / "retained/startup-manifest.json", 4*M)
            ]
            for prefix in ["artifacts/browser-emulator/"]:
                subtree = [item for item in package.infolist() if item.filename.startswith(prefix) and not item.is_dir()]
                require(0 < len(subtree) <= 256, "Unexpected retained proof subtree size")
                selected.extend((item.filename, source_root / item.filename, 128*M) for item in subtree)
        elif label == "kernel":
            destination = source_root / "artifacts/browser-kernel"
            selected = [(name, destination / relative, limit) for name, relative, limit in [
              ("kernel/bzImage", "bzImage", 32*M), ("kernel/linux.config", "linux.config", 2*M),
              ("kernel/upstream.config", "upstream.config", 2*M),
              ("kernel/ipc-baseline.config", "ipc-baseline.config", 2*M),
              ("kernel/acpi.config.diff", "acpi.config.diff", 65536),
              ("kernel/acpi-baseline.config", "acpi-baseline.config", 2*M),
              ("kernel/rng.config.diff", "rng.config.diff", 65536),
              ("kernel/COPYING", "COPYING", M), ("kernel/GPL-2.0", "GPL-2.0", M),
              ("kernel/SOURCE.txt", "SOURCE.txt", 65536), ("kernel/build.sha256", "build.sha256", 65536),
              ("metadata.json", "metadata.json", M), ("Kernel.Dockerfile", "Kernel.Dockerfile", 65536),
              ("source-commit.txt", "source-commit.txt", 128), ("workflow-commit.txt", "workflow-commit.txt", 128)
            ]]
        elif label == "native":
            # upload-artifact's common root is producer/artifacts, not producer.
            prefix = "browser-openfga/"
            subtree = [item for item in package.infolist() if item.filename.startswith(prefix) and not item.is_dir()]
            require(0 < len(subtree) <= 256, "Unexpected new OpenFGA proof subtree size")
            selected = [(item.filename, source_root / "artifacts" / item.filename, 128*M) for item in subtree]
            selected.extend((member, root / "native-evidence" / member, limit) for member, limit in [
              ("openfga10-verifier-input.json", 4*M),
              ("openfga10-build-verification.json", 4*M),
              ("browser-openfga-build.log", 64*M),
              ("openfga10-native-startup-timing.json", M),
              ("browser-bootstrap-proof/native-result.json", 4*M),
              ("browser-bootstrap-proof/openfga-build.json", 4*M)
            ])
        elif label == "bootstrap":
            expected = {"bootstrap": 128*M, "build.json": 4*M, "native-tests.log": 16*M, "SOURCE.txt": 65536}
            require({item.filename for item in package.infolist() if not item.is_dir()} == set(expected),
                    "Unexpected native bootstrap artifact members")
            selected = [(name, source_root / "artifacts/browser-bootstrap" / name, limit)
                        for name, limit in expected.items()]
        else:
            raise SystemExit("Unexpected archive role")
        for member, destination, limit in selected:
            selected_total += package.getinfo(member).file_size
            require(selected_total <= 768*M, "Selected artifact members exceeded budget")
            members[member] = extract(package, member, destination, limit)
    if label == "retained":
        manifest = json.loads((source_root / "browser-single/pages/runtime-manifest.json").read_text())
        nested = root / "runtime-source.zip"
        require(members["browser-output/wasm/runtime-source.zip"]["sha256"] == manifest["archive_sha256"],
                "Original converter archive changed")
        with zipfile.ZipFile(nested) as package:
            check_members(package)
            require(set(package.namelist()) == set(manifest["files"]), "Unexpected converter ZIP members")
            for member, expected in manifest["files"].items():
                require(member in ["out.js", "load.js", "arg-module.js", "qemu-system-x86_64.wasm", "qemu-system-x86_64.data"],
                        "Unexpected converter runtime member")
                actual = extract(package, member, root / "original-runtime" / member, expected["bytes"])
                require(actual == expected, "Original converter runtime SHA/size mismatch: " + member)
    elif label == "kernel":
        candidate = "716b77356dd4aab2747d4e37c5da723f3b7a764f"
        destination = source_root / "artifacts/browser-kernel"
        require((destination / "source-commit.txt").read_text().strip() == candidate and
                (destination / "workflow-commit.txt").read_text().strip() == head,
                "Kernel build source provenance changed")
        built = json.loads((destination / "metadata.json").read_text())
        require(built["source_commit"] == candidate and built["workflow_commit"] == head and
                built["run_id"] == str(run_id) and built["run_attempt"] == "1" and
                built["kernel_candidate_built"] is True and built["browser_acceptance_verified"] is False and
                built["entropy_delivery_verified"] is False and
                built["baseline_matches_previous_kernel_config"] is True and
                built["exactly_one_config_change"] is True and built["exact_rng_config_diff"] is True and
                built["previous_acpi_diff_preserved"] is True and built["kernel_hash_manifest_matches"] is True and
                built["source_checks_outcome"] == "success" and built["build_outcome"] == "success" and
                built["missing_config"] == [] and built["missing_files"] == [] and
                built["recipe_git_blob"] == "aeb75dc1f17528939559ffe8f8dd15746c0c2a0e",
                "Kernel build metadata identity mismatch")
    elif label == "native":
        inputs = json.loads((root / "native-evidence/openfga10-verifier-input.json").read_text())
        require(inputs["schemaVersion"] == 3 and inputs["experiment"] == "fresh_openfga10_two_walk_schema_reachability_and_bootstrap_deny" and
                inputs["producerSource"] == "bb8fed250bdee9931f7702ae2475d28e5e9eed8f" and
                inputs["runtimeSource"] == "444b069daf16e9606a0bbf1ea0aea7f4d61b97d8" and
                inputs["workflowCommit"] == head and inputs["runId"] == str(run_id) and inputs["runAttempt"] == "1" and
                inputs["repositoryRuntimeContract"] == "passed" and inputs["scriptSyntaxAndShellCheck"] == "passed" and
                inputs["browserAcceptanceVerified"] is False, "Fresh native source/input provenance mismatch")
        require(members["browser-openfga/openfga"] == native["binaries"]["openfga"],
                "The fresh OpenFGA binary bytes differ")
    elif label == "bootstrap":
        built = json.loads((source_root / "artifacts/browser-bootstrap/build.json").read_text())
        require(built["source_commit"] == "207d15fab685d3adb9c5d51a048982c2236ae75c" and
                built["packaging_commit"] == "4b7a1e1933083232fb8305a60a47bdd3c32e6689" and
                built["workflow_commit"] == head and built["run_id"] == str(run_id) and built["run_attempt"] == "1" and
                built["browser_acceptance_verified"] is False and built["native_tests"] == "passed" and
                built["empty_store_check"] == "one_real_higher_consistency_ownership_deny" and
                built["empty_store_check_default"] is False and built["browser_empty_store_check"] is True and
                built["empty_store_check_timeout_seconds"] == 5 and built["empty_store_check_tests"] == "passed" and
                {"bytes": built["binary_bytes"], "sha256": built["binary_sha256"]} == native["binaries"]["bootstrap"] and
                built["binary_bytes"] == members["bootstrap"]["bytes"] and
                built["binary_sha256"] == members["bootstrap"]["sha256"] and
                built["native_test_log_sha256"] == members["native-tests.log"]["sha256"],
                "Native bootstrap proof/binary identity mismatch")
    provenance = {"label": label, "run": run_id, "head": head, "artifact": aid, "artifact_name": name,
                  "archive": {"bytes": count, "sha256": sha}, "members": members}
    if recovery is not None: provenance["recovery"] = recovery
    if native_job is not None:
        provenance["native_run_attempt"] = run["run_attempt"]
        provenance["native_job"] = {key: native_job[key] for key in [
            "id", "run_id", "head_sha", "name", "status", "conclusion",
            "started_at", "completed_at", "steps"]}
    (root / (label + "-archive-provenance.json")).write_text(json.dumps(provenance, indent=2) + "\n")
    archive_path.unlink()
    print(json.dumps({"verified_archive": label, "artifact": aid, "bytes": count, "sha256": sha}), flush=True)


# Restore the very same successful emulator build used by the previous matrix.
# The original whole workflow failed its browser gate; only its build job passed.
aid = 14276419843
emulator_sha = "6fcd65f16a8a05207b7a15460ae2097ecafa00ee031fcc4c60bda3edf5e1ea30"
run_id = 43969702163
head = "a12b8799fed16e6efc93ab55498e8f165fb1a139"
producer = api("/actions/runs/" + str(run_id))
require(producer["id"] == run_id and producer["run_attempt"] == 1 and producer["head_sha"] == head
        and producer["head_branch"] == "diag/pr36-idle-worker-gc-20261117"
        and producer["path"] == ".github/workflows/pr36-idle-gc-gate.yml"
        and producer["status"] == "completed" and producer["conclusion"] == "failure"
        and producer["event"] == "push", "Original emulator producer run identity differs")
jobs = api("/actions/runs/" + str(run_id) + "/attempts/1/jobs?per_page=100")
require(jobs["total_count"] <= 100, "Emulator producer job list exceeded its bounded page")
matches = [job for job in jobs["jobs"] if job["name"] == "Build QEMU idle-worker GC candidate"]
require(len(matches) == 1, "Expected exactly one emulator build job")
job = matches[0]
require(job["id"] == 133658307719 and job["run_id"] == run_id
        and job["status"] == "completed" and job["conclusion"] == "success"
        and job["head_sha"] == head, "Original emulator build job did not succeed")
archive, count, recovery = obtain_archive("candidate-emulator", run_id, head, aid,
    "pr36-idle-gc-emulator", 16286946, emulator_sha)
destination = pathlib.Path("candidate-emulator")
destination.mkdir(exist_ok=False)
members, selected_total = {}, 0
with zipfile.ZipFile(archive) as package:
    check_members(package)
    require(len(package.infolist()) <= 512, "Emulator artifact member count exceeded budget")
    for item in package.infolist():
        if item.is_dir():
            continue
        require(0 <= item.file_size <= 128*M, "Invalid emulator artifact member size")
        selected_total += item.file_size
        require(selected_total <= 256*M, "Emulator artifact expansion exceeded budget")
        members[item.filename] = extract(package, item.filename,
            destination.joinpath(*pathlib.PurePosixPath(item.filename).parts), 128*M, allow_empty=True)
record = {"artifact": aid, "archive": {"bytes": count, "sha256": emulator_sha},
          "run": run_id, "attempt": 1, "workflowCommit": head, "buildJob": job,
          "members": members,
          "reuseConsumer": {"run": current_run, "attempt": 1, "workflowCommit": os.environ["GITHUB_SHA"]}}
if recovery is not None:
    record["recovery"] = recovery
(root / "candidate-emulator-archive-provenance.json").write_text(json.dumps(record, indent=2) + "\n")
archive.unlink()
print(json.dumps({"candidateArtifact": aid, "sha256": emulator_sha,
                  "members": len(members), "buildJob": job["id"],
                  "recoveredOriginalBytes": recovery is not None,
                  "browserAcceptanceVerified": False, "vmStarts": 0}), flush=True)
