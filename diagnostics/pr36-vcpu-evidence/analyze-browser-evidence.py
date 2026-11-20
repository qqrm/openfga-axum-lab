#!/usr/bin/env python3
"""Verify one PR36 browser artifact and summarize original, non-replayed events.

Usage: python3 analyze-browser-evidence.py ARTIFACT.zip --out REPORT.json
       [--expected-sha256 GITHUB_ARTIFACT_DIGEST]
All timestamps labeled hostReceivedAt are stream delivery timestamps. Recorded
service duration_ms values remain guest measurements. No timing comparison or
performance conclusion is made by this reader.
"""
import argparse
import bisect
import hashlib
import json
import pathlib
import re
import sys
import zipfile
from datetime import datetime


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def js_utf8(value):
    # JavaScript strings may carry split UTF-16 surrogate pairs across chunks.
    return value.encode("utf-16-le", "surrogatepass").decode("utf-16-le", "replace").encode("utf-8")


def parse_time(value):
    require(isinstance(value, str) and value.endswith("Z"), "Invalid host receive timestamp")
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def reconstruct(stream_bytes):
    batches, parts, ends = [], [], []
    expected, offset, previous_time = 1, 0, None
    for number, line in enumerate(stream_bytes.decode("utf-8").splitlines(), 1):
        require(bool(line), f"Empty JSONL source batch at line {number}")
        batch = json.loads(line)
        require(type(batch.get("firstSequence")) is int and batch["firstSequence"] == expected,
                f"Non-contiguous source sequence at JSONL line {number}: expected {expected}")
        texts = batch.get("texts")
        require(isinstance(texts, list) and len(texts) > 0 and all(isinstance(s, str) for s in texts),
                f"Invalid source text array at JSONL line {number}")
        received = parse_time(batch.get("receivedAt"))
        require(previous_time is None or received >= previous_time, "Host receive timestamps go backwards")
        previous_time = received
        text = "".join(texts)
        offset += len(text)
        expected += len(texts)
        parts.append(text)
        ends.append(offset)
        batches.append({"streamLine": number, "hostReceivedAt": batch["receivedAt"],
                        "firstSequence": batch["firstSequence"], "lastSequence": expected - 1,
                        "nonempty": bool(text)})
    return "".join(parts), batches, ends, expected - 1


def fields(line):
    result = {}
    for key, value in re.findall(r'([A-Za-z_][A-Za-z0-9_]*)=("(?:\\.|[^"\\])*"|[^\s]+)', line):
        if value.startswith('"'):
            value = json.loads(value)
        elif re.fullmatch(r"-?\d+", value):
            value = int(value)
        elif value in ("true", "false"):
            value = value == "true"
        result[key] = value
    return result


def analyze(filename, expected_sha=None):
    filename = pathlib.Path(filename)
    archive_digest = hashlib.sha256()
    with filename.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            archive_digest.update(block)
    archive_record = {"path": str(filename.resolve()), "bytes": filename.stat().st_size,
                      "sha256": archive_digest.hexdigest(), "expectedDigestVerified": expected_sha is not None}
    if expected_sha is not None:
        require(archive_record["sha256"] == expected_sha.removeprefix("sha256:"), "Artifact archive SHA-256 mismatch")
    with zipfile.ZipFile(filename) as package:
        names = package.namelist()
        require(len(names) == len(set(names)), "Duplicate ZIP member names")

        def read(name, limit=128 * 1024 * 1024):
            require(package.getinfo(name).file_size <= limit, f"Oversized evidence member: {name}")
            return package.read(name)

        def unique_suffix(suffix, optional=False):
            matching = [n for n in names if n.endswith(suffix)]
            if optional and not matching:
                return None
            require(len(matching) == 1, f"Expected one member ending {suffix!r}, got {len(matching)}")
            return matching[0]

        summaries = [n for n in names if re.search(r"/pr36-(?:vcpu|idle-gc)-gate/(?:chromium|firefox)-summary\.json$", n)]
        if not summaries:
            return {"archive": archive_record, "status": "no_browser_summary",
                    "browserExecutionVerified": False,
                    "availablePreflightEvidence": [n for n in names if any(s in n for s in ("preflight", "job-timing", "job-start", "system-chrome-smoke"))]}
        require(len(summaries) == 1, "Ambiguous browser summaries")
        summary_name = summaries[0]
        summary = json.loads(read(summary_name, 4 * 1024 * 1024))
        engine = summary["engine"]
        require(engine in ("chromium", "firefox"), "Unexpected engine")
        raw, members, verified = {}, {}, {}
        for suffix in ("-console.txt", "-stream.jsonl", ".json"):
            basename = engine + suffix
            name = unique_suffix("/browser-wasm/" + basename)
            data = read(name)
            actual = digest(data)
            require(actual == summary["files"].get(basename), f"Summary/evidence hash mismatch: {basename}")
            raw[suffix], members[basename], verified[basename] = data, name, actual
        result = json.loads(raw[".json"])
        require(result.get("engine") == engine, "Result engine mismatch")
        require(summary.get("browser") == result.get("browser"), "Result browser mismatch")
        for key in ("logCapture", "pageErrors", "passes"):
            require(summary.get(key) == result.get(key), f"Result/summary mismatch: {key}")
        text, batches, ends, chunks = reconstruct(raw["-stream.jsonl"])
        reconstructed = js_utf8(text)
        require(reconstructed == raw["-console.txt"], "Source JSONL does not reconstruct the exact console")
        capture = result.get("logCapture") or {}
        require(capture.get("format") == "browser-lab-log-batches-v1", "Unexpected source capture format")
        for key, actual in (("chunks", chunks), ("batches", len(batches)), ("bytes", len(reconstructed)), ("expectedSequence", chunks)):
            require(capture.get(key) == actual, f"Capture counter mismatch: {key}")

        # Preserve line and batch references. For each complete source line use
        # the receive timestamp of the batch that supplied its final character.
        rows, offset = [], 0
        # A bare CR is a serial formatting character, not a new log record.
        for number, original in enumerate(text.split("\n"), 1):
            line_end = offset + len(original)
            if line_end < len(text):
                line_end += 1
            if line_end > offset:
                batch_index = bisect.bisect_right(ends, line_end - 1)
                batch = batches[batch_index]
                rows.append({"consoleLine": number, "characterOffset": offset,
                             "streamLine": batch["streamLine"], "hostReceivedAt": batch["hostReceivedAt"],
                             "line": original.replace("\r", "")})
            offset = line_end
        tail = next((i for i, row in enumerate(rows) if row["line"].startswith("Guest diagnostic:")), len(rows))
        original_rows = rows[:tail]
        diagnostic_cutoff = rows[tail] if tail < len(rows) else None
        phases, service_events, service_errors, protocol, seen = [], [], [], [], set()
        phase_patterns = (
            ("guest_boot", r"^LAB\|boot$"),
            ("executables_staged", r"\[browser-guest\] executables staged;"),
            ("postgres_init", r"\[browser-single\] initializing isolated PostgreSQL"),
            ("postgres_started", r"^server started$"),
            ("openfga_migrate_start", r"\[browser-single\] migrating and starting real OpenFGA"),
            ("openfga_listener_wait", r"\[browser-single\] waiting for the OpenFGA HTTP listener"),
            ("model_bind_start", r"\[browser-single\] binding the pinned"),
            ("axum_start", r"\[browser-single\].*(?:starting.*Axum|starting the Rust|starting Axum)"),
            ("listening", r"Listening on"),
        )
        first_phases = set()
        for row in original_rows:
            line = row["line"]
            if line in seen:
                continue
            seen.add(line)
            for kind, pattern in phase_patterns:
                if kind not in first_phases and re.search(pattern, line):
                    phases.append({"kind": kind, **row})
                    first_phases.add(kind)
            event = re.search(r"\b(warmup_start|warmup_cycle|warmup_end|grpc_dial_start|grpc_dial_end|rpc_start|rpc_end|http_start|http_end|model_resolver_transfer)\b", line)
            if event and "[browser-openfga]" in line:
                kind, values = event.group(1), fields(line)
                include = not kind.startswith(("rpc_", "http_"))
                if kind.startswith("rpc_"):
                    include = (values.get("method") == "/openfga.v1.OpenFGAService/Check"
                               or (kind == "rpc_end" and values.get("status") not in (None, "OK")))
                elif kind.startswith("http_"):
                    include = isinstance(values.get("path"), str) and values["path"].endswith("/check")
                if include:
                    guest_time = re.search(r"\d{4}/\d{2}/\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)?", line)
                    service_events.append({"kind": kind, **row, "fields": values,
                                           "guestLoggedAt": guest_time.group() if guest_time else None})
            if re.search(r"(?:^Error:|^ERROR:|\bFATAL\b|\bPANIC\b|service.*(?:exit|failed)|required.*(?:exit|failed))", line):
                service_errors.append(row)
            if line.startswith("LAB|"):
                protocol.append(row)
        terminals = [row for row in rows if row["line"] == "LAB|boot-failed"][:1]
        check_ends = [event for event in service_events if event["kind"] == "rpc_end"
                      and event["fields"].get("method") == "/openfga.v1.OpenFGAService/Check"]
        other_rpc_failures = [event for event in service_events if event["kind"] == "rpc_end"
                              and event["fields"].get("method") != "/openfga.v1.OpenFGAService/Check"
                              and event["fields"].get("status") not in (None, "OK")]
        guest = result.get("guest") or {}
        passes = result.get("passes")
        vector_ok = isinstance(passes, list) and len(passes) == 2 and all(
            p.get("id") == i + 1 and p.get("accepted") is True and p.get("statuses") == [403, 204, 200, 204, 403]
            for i, p in enumerate(passes))
        vector_ok = vector_ok and passes == guest.get("passes") and guest.get("checks") == 2
        acceptance = (vector_ok and result.get("passed") is True and guest.get("booted") is True
                      and guest.get("ready") is True and guest.get("accepted") is True and not guest.get("error")
                      and result.get("pageErrors") == [] and capture.get("complete") is True)
        if summary.get("twoRealSequencesPassed") is True:
            require(acceptance, "Summary claims acceptance without two verified real vectors")
        online = [r["line"].split(": ", 1)[1] for r in original_rows if r["line"].startswith("[browser-guest] confirmed online CPUs: ")]
        allowed = [r["line"].split(": ", 1)[1] for r in original_rows if r["line"].startswith("[browser-guest] confirmed service CPU availability: ")]
        requested = summary.get("requestedVirtualCPUs", guest.get("vm", {}).get("cpus"))
        expected_list = {1: "0", 2: "0-1"}.get(requested)
        vm = guest.get("vm") or {}
        topology_ok = (expected_list is not None and vm.get("cpus") == requested
                       and vm.get("cpuTopology") == f"{requested},sockets={requested},cores=1,threads=1"
                       and bool(online) and all(v == expected_list for v in online)
                       and bool(allowed) and all(v == expected_list for v in allowed))

        def optional_json(suffix):
            name = unique_suffix(suffix, optional=True)
            return json.loads(read(name, 4 * 1024 * 1024)) if name else None

        host_cpu_name = unique_suffix("/host-cpu.txt", optional=True)
        cpu_fields = {}
        if host_cpu_name:
            for line in read(host_cpu_name, 1024 * 1024).decode().splitlines():
                if ":" in line:
                    key, value = line.split(":", 1)
                    if key in ("Architecture", "CPU(s)", "Model name", "CPU family", "Model", "Stepping", "Thread(s) per core", "Core(s) per socket", "Socket(s)", "Hypervisor vendor", "Virtualization type"):
                        cpu_fields[key] = value.strip()
        host_memory_name = unique_suffix("/host-memory.txt", optional=True)
        mem = read(host_memory_name, 1024 * 1024).decode() if host_memory_name else ""
        mem_total = re.search(r"^MemTotal:\s+(\d+) kB$", mem, re.M)
        nonempty_batches = [batch for batch in batches if batch["nonempty"]]
        return {
            "schemaVersion": 1, "status": "verified_evidence", "archive": archive_record,
            "summaryMember": summary_name, "evidenceMembers": members, "verifiedHashes": verified,
            "integrity": {"allThreeHashesMatchSummary": True, "sequenceContiguous": True,
                          "exactConsoleReconstruction": True, "captureCountersMatch": True},
            "engine": engine, "browser": result.get("browser"), "workflowCommit": summary.get("workflowCommit"),
            "sourceCommit": summary.get("sourceCommit"), "emulatorSource": summary.get("candidateSource"),
            "producer": summary.get("emulatorOverlay", {}).get("producer"),
            "job": summary.get("jobBudget"), "host": {"cpu": cpu_fields, "memoryTotalKiB": int(mem_total.group(1)) if mem_total else None},
            "chromeSmoke": summary.get("chromeSmoke"), "playwrightLock": optional_json("/materialization.json").get("sourceFiles", {}).get("e2e/package-lock.json") if unique_suffix("/materialization.json", optional=True) else None,
            "topology": {"requestedVirtualCPUs": requested, "vm": vm, "originalOnlineMarkers": online,
                         "originalServiceAvailabilityMarkers": allowed, "matches": topology_ok,
                         "checkerObservation": summary.get("observedTopology")},
            "capture": capture, "pageErrors": result.get("pageErrors"),
            "acceptance": {"twoRealVectorsVerified": acceptance, "checkerClaimsPass": summary.get("twoRealSequencesPassed"),
                           "fullGateVerified": acceptance and topology_ok and summary.get("twoRealSequencesPassed") is True and summary.get("outcome", {}).get("code") == 0,
                           "passes": passes, "guestPasses": guest.get("passes"), "booted": guest.get("booted"),
                           "ready": guest.get("ready"), "accepted": guest.get("accepted"), "checks": guest.get("checks")},
            "sourceTimeline": {"firstNonemptyBatch": nonempty_batches[0] if nonempty_batches else None,
                               "lastNonemptyBatch": nonempty_batches[-1] if nonempty_batches else None,
                               "resultStartedAt": result.get("startedAt"), "resultFinishedAt": result.get("finishedAt")},
            "diagnosticReplayCutoff": diagnostic_cutoff,
            "eventScope": "Only unique full original lines before the first Guest diagnostic replay; terminal LAB|boot-failed is recorded separately. Guest service duration_ms and host receipt timestamps are not interchangeable.",
            "phases": phases, "openfgaEvents": service_events, "checkOutcomes": check_ends,
            "rpcFailures": other_rpc_failures,
            "warmupOutcomes": [e for e in service_events if e["kind"] == "warmup_end"],
            "originalServiceErrors": service_errors, "originalProtocolMarkers": protocol,
            "terminalMarkers": terminals, "lastOriginalServiceEvent": max(phases + service_events + service_errors, key=lambda e: e["consoleLine"], default=None),
            "failure": {"guest": guest.get("error"), "browserChecker": result.get("failure"),
                        "validation": summary.get("validationError"), "outcome": summary.get("outcome")},
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive")
    parser.add_argument("--out")
    parser.add_argument("--expected-sha256")
    args = parser.parse_args()
    try:
        report = analyze(args.archive, args.expected_sha256)
    except (ValueError, KeyError, OSError, zipfile.BadZipFile) as error:
        print(json.dumps({"status": "verification_failed", "archive": args.archive, "error": str(error)}), file=sys.stderr)
        return 2
    if args.out:
        pathlib.Path(args.out).write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
        compact = {key: report.get(key) for key in ("status", "engine", "browser", "integrity", "capture", "pageErrors", "acceptance")}
        compact["failure"] = {key: report.get("failure", {}).get(key) for key in ("guest", "validation", "outcome")}
        compact["topology"] = {key: report.get("topology", {}).get(key) for key in ("requestedVirtualCPUs", "originalOnlineMarkers", "originalServiceAvailabilityMarkers", "matches")}
        compact["checks"] = [event["fields"] for event in report.get("checkOutcomes", [])]
        compact["rpcFailures"] = [event["fields"] for event in report.get("rpcFailures", [])]
        compact["warmups"] = [event["fields"] for event in report.get("warmupOutcomes", [])]
        compact["report"] = str(pathlib.Path(args.out).resolve())
        print(json.dumps(compact, ensure_ascii=False))
    else:
        print(json.dumps(report, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
