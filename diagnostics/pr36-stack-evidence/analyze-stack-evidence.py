#!/usr/bin/env python3
"""Audit one datastore arm from a retained PR36 full-stack browser ZIP.

python3 analyze-stack-evidence.py EVIDENCE.zip --datastore memory \
    --expected-sha256 SHA256 --out analysis.json

This reader launches no browser/guest and reads no network. Host timestamps are
batch receipt times, not CPU execution samples. Request subjects, objects and
relations are deliberately not inferred from the metadata-only transport trace.
"""
import argparse
import bisect
from collections import Counter, defaultdict
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import statistics
import zipfile


SOURCE = "444b069daf16e9606a0bbf1ea0aea7f4d61b97d8"
EMULATOR = "6f35f24b7ca345cf47c873becee6a41ef94e65a9"
FGA = "/openfga.v1.OpenFGAService/"
LIMITS = {"chromium-console.txt": 16 * 1024**2,
          "chromium-stream.jsonl": 128 * 1024**2,
          "chromium.json": 2 * 1024**2,
          "chromium-console-tail.txt": 2 * 1024**2,
          "chromium.png": 16 * 1024**2}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def strict_json(value):
    def pairs(items):
        result = {}
        for key, item in items:
            require(key not in result, "Duplicate JSON key: " + key)
            result[key] = item
        return result

    def invalid_constant(value):
        raise ValueError("Invalid JSON constant: " + value)

    return json.loads(value, object_pairs_hook=pairs, parse_constant=invalid_constant)


def digest(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def js_utf8(value):
    # TextEncoder handles lone UTF-16 surrogates per chunk; joining the full
    # transcript can subsequently reunite a pair. Preserve both byte counts.
    return value.encode("utf-16-le", "surrogatepass").decode("utf-16-le", "replace").encode("utf-8")


def timestamp(value):
    require(isinstance(value, str) and value.endswith("Z"), "Invalid host timestamp")
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def milliseconds(start, end):
    return round((timestamp(end) - timestamp(start)).total_seconds() * 1000, 3)


def fields(text):
    result = {}
    for key, value in re.findall(r'([A-Za-z_][A-Za-z0-9_]*)=("(?:\\.|[^"\\])*"|[^\s]+)', text):
        require(key not in result, "Repeated transport metadata key: " + key)
        if value.startswith('"'):
            value = strict_json(value)
        elif re.fullmatch(r"-?\d+", value):
            value = int(value)
        elif value in ("true", "false"):
            value = value == "true"
        result[key] = value
    return result


def stats(values):
    if not values:
        return {"count": 0}
    ordered = sorted(values)
    return {"count": len(values), "min": min(values), "median": statistics.median(values),
            "mean": round(statistics.mean(values), 3), "max": max(values),
            "p90": ordered[int((len(values) - 1) * .90)],
            "p95": ordered[int((len(values) - 1) * .95)],
            "p99": ordered[int((len(values) - 1) * .99)],
            "quantileMethod": "sorted[floor((count-1)*p)]",
            "atLeast1000": sum(value >= 1000 for value in values),
            "atLeast2000": sum(value >= 2000 for value in values)}


def reconstruct(stream, console, metadata):
    text = stream.decode("utf-8")
    require(not text or text.endswith("\n"), "Truncated final JSONL batch")
    raw, lines, telemetry, batches = [], [], [], []
    chunks = byte_count = 0
    previous_time = None
    pending, first, last = "", None, None
    for number, encoded in enumerate(text[:-1].split("\n") if text else [], 1):
        require(bool(encoded), "Empty JSONL batch")
        batch = strict_json(encoded)
        require(set(batch) == {"firstSequence", "receivedAt", "texts"}, "Unexpected batch fields")
        require(type(batch["firstSequence"]) is int and batch["firstSequence"] == chunks + 1,
                "Non-contiguous original source sequence")
        received = timestamp(batch["receivedAt"])
        require(previous_time is None or received >= previous_time, "Host receipt timestamps went backwards")
        previous_time = received
        require(isinstance(batch["texts"], list) and 0 < len(batch["texts"]) <= 1024, "Invalid batch size")
        batches.append({"streamLine": number, "firstSequence": chunks + 1,
                        "hostReceivedAt": batch["receivedAt"]})
        for chunk in batch["texts"]:
            require(isinstance(chunk, str), "Non-string source chunk")
            chunks += 1
            byte_count += len(js_utf8(chunk))
            require(chunks <= 1_000_000 and byte_count <= 16 * 1024**2, "Production capture bound exceeded")
            raw.append(chunk)
            position = {"sequence": chunks, "streamLine": number, "hostReceivedAt": batch["receivedAt"]}
            if chunk.startswith("[browser-qemu] "):
                match = re.fullmatch(r"\[browser-qemu\] (\{[^\r\n]*\})\n", chunk)
                require(match is not None, "Malformed or mixed QEMU telemetry chunk")
                event = strict_json(match.group(1))
                require(isinstance(event, dict) and isinstance(event.get("event"), str), "Invalid QEMU event")
                telemetry.append({**position, "bytes": len(js_utf8(chunk)), "value": event})
                continue
            for character in chunk:
                first = first or position
                last = position
                pending += character
                if character == "\n":
                    lines.append({"sourceLine": len(lines) + 1,
                                  "firstSequence": first["sequence"], "lastSequence": last["sequence"],
                                  "firstStreamLine": first["streamLine"], "lastStreamLine": last["streamLine"],
                                  "firstHostReceivedAt": first["hostReceivedAt"],
                                  "lastHostReceivedAt": last["hostReceivedAt"],
                                  "text": pending[:-1].replace("\r", "")})
                    pending, first, last = "", None, None
    require(js_utf8("".join(raw)) == console, "Original chunks do not reconstruct the exact console")
    require(metadata.get("format") == "browser-lab-log-batches-v1" and metadata.get("complete") is True,
            "Incomplete or unsupported production capture")
    require("error" not in metadata, "Production capture recorded an error")
    for key, value in {"chunks": chunks, "expectedSequence": chunks, "batches": len(batches), "bytes": byte_count}.items():
        require(type(metadata.get(key)) is int and metadata[key] == value, "Capture counter differs: " + key)
    normalized_pending = pending.replace("\r", "")
    report = {"verified": True, "chunks": chunks, "batches": len(batches), "bytes": byte_count,
              "telemetryChunks": len(telemetry), "telemetryBytes": sum(row["bytes"] for row in telemetry),
              "consoleBytes": len(console), "streamBytes": len(stream),
              "partialProtocolRecord": "LAB|" in normalized_pending or
              bool(normalized_pending and "LAB|".startswith(normalized_pending))}
    return lines, telemetry, batches, report, normalized_pending


def paired(events, family):
    # There are no request IDs in these traces. Only associate an unambiguous,
    # non-overlapping start/end of the same method (and HTTP path). Overlapping
    # groups remain explicitly unpaired instead of assuming FIFO ordering.
    active, ambiguous, completed = defaultdict(list), set(), []
    for event in events:
        kind, values = event["kind"], event["fields"]
        if kind not in (family + "_start", family + "_end"):
            continue
        key = (values.get("method"), values.get("path") if family == "http" else None)
        if kind.endswith("_start"):
            active[key].append(event)
            if len(active[key]) > 1:
                ambiguous.add(key)
        else:
            start = active[key][0] if len(active[key]) == 1 and key not in ambiguous else None
            completed.append({"start": start, "end": event,
                              "association": "unique non-overlapping start/end" if start else "missing or overlapping start"})
            if active[key]:
                active[key].pop(0)
            if not active[key]:
                ambiguous.discard(key)
    unfinished = [event for group in active.values() for event in group]
    return completed, unfinished


def analyze(archive, datastore, expected):
    expected = expected.removeprefix("sha256:").lower()
    require(re.fullmatch(r"[0-9a-f]{64}", expected) is not None, "Expected a SHA-256 digest")
    hasher = hashlib.sha256()
    with archive.open("rb") as source:
        for block in iter(lambda: source.read(1024**2), b""):
            hasher.update(block)
    require(hasher.hexdigest() == expected, "ZIP SHA-256 mismatch")
    with zipfile.ZipFile(archive) as package:
        names = package.namelist()
        require(len(names) == len(set(names)), "Duplicate ZIP member names")

        def unique(suffix, optional=False):
            matching = [name for name in names if name == suffix or name.endswith("/" + suffix)]
            if optional and not matching:
                return None
            require(len(matching) == 1, "Missing or ambiguous ZIP member: " + suffix)
            return matching[0]

        def read(name, limit):
            info = package.getinfo(name)
            require(not info.is_dir() and info.file_size <= limit, "Invalid or oversized member: " + name)
            return package.read(name)

        summary_name = unique("pr36-stack-continuation/" + datastore + "/summary.json")
        directory = summary_name.rsplit("/", 1)[0] + "/"
        summary_bytes = read(summary_name, 8 * 1024**2)
        summary = strict_json(summary_bytes)
        for key, value in {"engine": "chromium", "datastoreEngine": datastore, "sourceCommit": SOURCE,
                           "emulatorSource": EMULATOR, "checkerLaunched": True, "checkerExited": True,
                           "logCaptureComplete": True, "productionGatePassed": False}.items():
            require(summary.get(key) == value, "Unexpected summary identity/completion field: " + key)
        require(type(summary.get("runId")) is int and summary["runId"] > 0, "Invalid run identity")
        require(type(summary.get("runAttempt")) is int and summary["runAttempt"] == 1, "Unexpected run attempt")
        require(summary["job"]["run_id"] == summary["runId"] and
                summary["job"]["head_sha"] == summary["workflowCommit"] and
                summary["job"]["name"] == "Browser full-stack datastore comparison", "Job identity mismatch")
        raw, verified = {}, {}
        for basename, expected_record in summary["files"].items():
            require(basename in LIMITS, "Unexpected archived result filename")
            raw[basename] = read(directory + basename, LIMITS[basename])
            require(digest(raw[basename]) == expected_record, "Summary hash mismatch: " + basename)
            verified[basename] = expected_record
        require(all(name in raw for name in ("chromium.json", "chromium-console.txt", "chromium-stream.jsonl")),
                "Required source evidence is absent")
        result = strict_json(raw["chromium.json"])
        require(result == summary["result"], "Retained result differs from wrapper result")
        require(result.get("engine") == "chromium" and result.get("browser") == summary["browser"], "Browser identity differs")
        overlay_bytes = read(directory + "stack-overlay.json", 4 * 1024**2)
        require(digest(overlay_bytes) == summary["stackOverlay"], "Stack overlay hash mismatch")
        overlay = strict_json(overlay_bytes)
        require(overlay.get("datastoreEngine") == datastore and overlay.get("baselineSource") == SOURCE,
                "Stack overlay identity differs")
        host_name = unique("pr36-vcpu-gate/host-cpu.txt", optional=True)
        host_bytes = read(host_name, 256 * 1024) if host_name else None
    lines, telemetry, batches, capture, pending = reconstruct(
        raw["chromium-stream.jsonl"], raw["chromium-console.txt"], result["logCapture"])
    require(capture == summary["sourceCapture"], "Independent reconstruction differs from wrapper counters")
    cutoff_index = next((i for i, line in enumerate(lines) if line["text"].startswith("Guest diagnostic:")), len(lines))
    cutoff = lines[cutoff_index] if cutoff_index < len(lines) else None
    original = lines[:cutoff_index]
    original_telemetry = [row for row in telemetry if cutoff is None or row["sequence"] < cutoff["firstSequence"]]
    telemetry_sequences = [row["sequence"] for row in original_telemetry]
    phase, observations, events, offsets = "service_startup", [], [], []
    for line in original:
        text = line["text"]
        if text.startswith("STACK|"):
            match = re.fullmatch(r"STACK\|(phase|entrypoint-exit|guest-failure)\|(\{.*\})", text)
            require(match is not None, "Malformed original STACK record")
            value = strict_json(match.group(2))
            if "datastore" in value:
                require(value["datastore"] == datastore, "Observed datastore differs")
            observations.append({**line, "kind": match.group(1), "value": value})
            if match.group(1) == "phase":
                if value["phase"] == "bootstrap-start":
                    phase = "bootstrap"
                elif value["phase"] == "bootstrap-complete":
                    phase = "after_bootstrap"
                elif value["phase"] == "app-start":
                    phase = "application_startup"
        elif text == "LAB|ready":
            phase = "application_acceptance"
        if not text.startswith("[browser-openfga] "):
            continue
        match = re.match(r"\[browser-openfga\] (\d{4}/\d{2}/\d{2} \d{2}:\d{2}:\d{2}\.\d+) (\w+) ", text)
        if not match:
            continue
        event = {**line, "phase": phase, "guestPrintedAt": match.group(1), "kind": match.group(2),
                 "fields": fields(text[match.end():])}
        events.append(event)
        if event["kind"].startswith(("rpc_", "http_")):
            guest = datetime.strptime(match.group(1), "%Y/%m/%d %H:%M:%S.%f").replace(tzinfo=timezone.utc)
            offsets.append((timestamp(line["firstHostReceivedAt"]) - guest).total_seconds())
    rpc_pairs, unfinished_rpc = paired(events, "rpc")
    http_pairs, unfinished_http = paired(events, "http")
    phase_ordinals = Counter()
    rpcs = []
    for pair in rpc_pairs:
        start, end = pair["start"], pair["end"]
        item = {**pair, "phase": (start or end)["phase"], "method": end["fields"]["method"],
                "status": end["fields"].get("status"), "guestDurationMs": end["fields"].get("duration_ms"),
                "http": None, "hostRpcReceiptIntervalMs": None}
        if start:
            item["hostRpcReceiptIntervalMs"] = milliseconds(start["firstHostReceivedAt"], end["firstHostReceivedAt"])
            item["hostIntervalMinusGuestDurationMs"] = round(item["hostRpcReceiptIntervalMs"] - item["guestDurationMs"], 3)
            item["qemuTelemetryChunksDuringRpc"] = bisect.bisect_right(telemetry_sequences, end["lastSequence"]) - bisect.bisect_left(telemetry_sequences, start["firstSequence"])
            containing = [candidate for candidate in http_pairs if candidate["start"] is not None and
                          candidate["start"]["firstSequence"] < start["firstSequence"] and
                          candidate["end"]["lastSequence"] > end["lastSequence"]]
            if len(containing) == 1:
                http = containing[0]
                item["http"] = {**http, "guestDurationMs": http["end"]["fields"].get("duration_ms"),
                                "hostReceiptIntervalMs": milliseconds(http["start"]["firstHostReceivedAt"], http["end"]["firstHostReceivedAt"])}
        if item["method"] == FGA + "Check":
            phase_ordinals[item["phase"]] += 1
            item["checkOrdinalWithinPhase"] = phase_ordinals[item["phase"]]
        rpcs.append(item)
    checks = [item for item in rpcs if item["method"] == FGA + "Check"]
    writes = [item for item in rpcs if item["method"] == FGA + "Write"]
    failures = [item for item in rpcs if item["status"] != "OK"]
    phase_stats = {}
    for name in sorted({item["phase"] for item in checks}):
        selected = [item for item in checks if item["phase"] == name]
        successful = [item for item in selected if item["status"] == "OK"]
        phase_stats[name] = {"completed": len(selected), "successful": len(successful),
                             "failed": len(selected) - len(successful),
                             "successfulGuestRpcMilliseconds": stats([item["guestDurationMs"] for item in successful]),
                             "successfulHostRpcReceiptMilliseconds": stats([item["hostRpcReceiptIntervalMs"] for item in successful if item["hostRpcReceiptIntervalMs"] is not None])}
    first_failure = failures[0] if failures else None
    timing_focus = None
    if first_failure and first_failure["start"]:
        before = [row for row in original_telemetry if row["sequence"] < first_failure["start"]["firstSequence"]]
        last = before[-1] if before else None
        timing_focus = {"method": first_failure["method"], "phase": first_failure["phase"],
                        "checkOrdinalWithinPhase": first_failure.get("checkOrdinalWithinPhase"),
                        "lastQemuTelemetryBeforeRpcStart": last,
                        "lastTelemetryAgeAtRpcStartMs": milliseconds(last["hostReceivedAt"], first_failure["start"]["firstHostReceivedAt"]) if last else None,
                        "qemuTelemetryChunksDuringRpc": first_failure["qemuTelemetryChunksDuringRpc"]}
    core_last = {}
    for row in original_telemetry:
        if "core_id" in row["value"]:
            core_last[str(row["value"]["core_id"])] = row
    source_gaps = [{"milliseconds": milliseconds(a["hostReceivedAt"], b["hostReceivedAt"]),
                    "from": a, "to": b} for a, b in zip(batches, batches[1:])
                   if cutoff is None or b["firstSequence"] < cutoff["firstSequence"]]
    host = None
    if host_bytes is not None:
        host = {"member": host_name, **digest(host_bytes), "archiveBound": True,
                "fields": {key.strip(): value.strip() for key, value in
                           (line.split(":", 1) for line in host_bytes.decode("utf-8").splitlines() if ":" in line)
                           if key.strip() in ("Architecture", "CPU(s)", "On-line CPU(s) list", "Model name", "Hypervisor vendor")}}
    return {"schemaVersion": 1, "reader": "analyze-stack-evidence.py", "datastoreEngine": datastore,
            "archive": {"path": str(archive.resolve()), "bytes": archive.stat().st_size, "sha256": expected, "expectedDigestVerified": True},
            "summary": {"member": summary_name, **digest(summary_bytes)}, "rawFilesVerified": verified,
            "stackOverlayHashVerified": digest(overlay_bytes), "sourceCapture": capture,
            "identity": {key: summary.get(key) for key in ("engine", "browser", "sourceCommit", "emulatorSource", "workflowCommit", "runId", "runAttempt", "job")},
            "host": host, "resultStartedAt": result.get("startedAt"), "resultFinishedAt": result.get("finishedAt"),
            "applicationAcceptancePassed": summary["applicationAcceptancePassed"], "productionGatePassed": False,
            "comparisonContinuationPermitted": summary.get("comparisonContinuationPermitted"),
            "vm": result.get("guest", {}).get("vm"), "observedTopology": summary.get("observedTopology"),
            "passes": result.get("passes"), "pageErrors": result.get("pageErrors"),
            "guestError": result.get("guest", {}).get("error"), "failure": result.get("failure"),
            "diagnosticReplayBegins": cutoff, "completeSourceLinesBeforeReplay": len(original),
            "trailingUnterminatedText": pending, "phaseObservations": observations,
            "transportEventCounts": dict(Counter(event["kind"] for event in events)),
            "checkCountsByPhase": phase_stats, "checkRpcs": checks, "writeRpcs": writes,
            "rpcFailures": failures, "unfinishedRpcStarts": unfinished_rpc, "unfinishedHttpStarts": unfinished_http,
            "rpcAssociationAmbiguities": sum(pair["start"] is None for pair in rpc_pairs),
            "httpAssociationAmbiguities": sum(pair["start"] is None for pair in http_pairs),
            "nonRpcServiceEvents": [event for event in events if not event["kind"].startswith(("rpc_", "http_"))],
            "firstFailureTimingContext": timing_focus,
            "hostMinusGuestPrintedTimestampOffsetSeconds": {"min": min(offsets), "median": statistics.median(offsets), "max": max(offsets)} if offsets else None,
            "qemuTelemetry": {"originalEventCount": len(original_telemetry),
                              "eventKinds": dict(Counter(row["value"]["event"] for row in original_telemetry)),
                              "lastOriginalPerCore": core_last},
            "largestOriginalSourceReceiptGaps": sorted(source_gaps, key=lambda gap: gap["milliseconds"], reverse=True)[:10],
            "interpretationLimits": [
                "Transport results stop before the first Guest diagnostic replay; replayed tails are never new operations.",
                "Only exact /openfga.v1.OpenFGAService/Check RPCs count as authorization Checks; health Checks and other methods remain separate.",
                "Phase and ordinal come from original STACK/LAB markers and observed request order; no subject/object/relation identity is inferred.",
                "Successful authorization results are not printed by these metadata-only traces; RPC OK is not proof of an allow decision.",
                "Host times describe batch receipt, not guest execution time. Clock offsets are not transport latency, and matching intervals do not distinguish compute from scheduling pauses.",
                "No request ID is logged: starts/ends and containing HTTP spans are associated only when uniquely non-overlapping.",
                "QEMU registered snapshots occur every 1024 registrations; they are not a heartbeat. Their absence and stale GC counters do not establish a pause, budget saturation, or root cause.",
                "This offline reader verifies retained evidence, not a rerun or a successful production gate.",
            ]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--datastore", required=True, choices=("memory", "postgres"))
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    report = analyze(args.archive, args.datastore, args.expected_sha256)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"out": str(args.out.resolve()), "datastore": args.datastore,
                      "chunks": report["sourceCapture"]["chunks"],
                      "checksByPhase": report["checkCountsByPhase"],
                      "writes": len(report["writeRpcs"]),
                      "failures": [{"method": item["method"], "phase": item["phase"],
                                    "checkOrdinalWithinPhase": item.get("checkOrdinalWithinPhase"),
                                    "status": item["status"], "guestDurationMs": item["guestDurationMs"]}
                                   for item in report["rpcFailures"]]}, ensure_ascii=False))
