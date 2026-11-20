#!/usr/bin/env python3
"""Read-only browser VM telemetry reader; no guest or browser execution.

Example: python3 analyze-vcpu-telemetry.py EVIDENCE.zip --engine firefox --output RESULT.json
"""
import argparse
import bisect
import hashlib
import json
from pathlib import Path
import re
from datetime import datetime
import zipfile


def record(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def epoch_ms(value):
    return round(datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp() * 1000)


def elapsed(first, second):
    return epoch_ms(second) - epoch_ms(first)


def analyze(archive, engine):
    with zipfile.ZipFile(archive) as package:
        def read_suffix(suffix, limit, optional=False):
            matches = [item for item in package.infolist() if item.filename.endswith(suffix) and not item.is_dir()]
            if optional and not matches:
                return None
            assert len(matches) == 1, ("Expected exactly one evidence member", suffix, len(matches))
            item = matches[0]
            assert 0 < item.file_size <= limit, ("Evidence size outside bounds", item.filename, item.file_size)
            data = package.read(item)
            assert len(data) == item.file_size
            return data

        prefix = "/artifacts/browser-wasm/" + engine
        raw = {
            engine + "-stream.jsonl": read_suffix(prefix + "-stream.jsonl", 128 * 1024**2),
            engine + "-console.txt": read_suffix(prefix + "-console.txt", 16 * 1024**2),
            engine + ".json": read_suffix(prefix + ".json", 2 * 1024**2),
        }
        summary_bytes = read_suffix("/pr36-vcpu-gate/" + engine + "-summary.json", 4 * 1024**2, optional=True)
        if summary_bytes is None:
            summary_bytes = read_suffix("/pr36-idle-gc-gate/" + engine + "-summary.json", 4 * 1024**2, optional=True)
        summary = json.loads(summary_bytes) if summary_bytes else None
        raw_records = {name: record(data) for name, data in raw.items()}
        if summary:
            for name, expected in raw_records.items():
                assert summary["files"][name] == expected, ("Summary/raw identity mismatch", name)

    result = json.loads(raw[engine + ".json"])
    assert result["engine"] == engine
    batches = [json.loads(line) for line in raw[engine + "-stream.jsonl"].splitlines()]
    pieces, segments, nonempty = [], [], []
    sequence, offset = 1, 0
    previous_timestamp = None
    for number, batch in enumerate(batches, 1):
        assert batch["firstSequence"] == sequence, ("Missing/duplicate stream chunk", number, sequence, batch["firstSequence"])
        assert isinstance(batch["texts"], list) and batch["texts"]
        assert all(isinstance(part, str) for part in batch["texts"])
        timestamp = batch["receivedAt"]
        milliseconds = epoch_ms(timestamp)
        assert previous_timestamp is None or milliseconds >= previous_timestamp, "Host receipt timestamps regress"
        previous_timestamp = milliseconds
        sequence += len(batch["texts"])
        text = "".join(batch["texts"])
        if text:
            pieces.append(text)
            start = offset
            offset += len(text)
            point = {"streamLine": number, "receivedAt": timestamp, "startCharacter": start, "endCharacter": offset}
            segments.append(point)
            nonempty.append(point)
    console = "".join(pieces)
    assert console.encode() == raw[engine + "-console.txt"], "Stream reconstruction differs from complete console"
    capture = result["logCapture"]
    assert capture["complete"] is True, "Incomplete source capture; do not treat this as complete telemetry"
    assert capture["chunks"] == sequence - 1 == capture["expectedSequence"]
    assert capture["batches"] == len(batches)
    assert capture["bytes"] == len(raw[engine + "-console.txt"])
    assert nonempty
    segment_ends = [segment["endCharacter"] for segment in segments]

    def position(end_character):
        index = bisect.bisect_left(segment_ends, end_character)
        segment = segments[index]
        return {"receivedAt": segment["receivedAt"], "streamLine": segment["streamLine"], "endCharacter": end_character}

    diagnostic = re.search(r"(?:^|[\r\n])Guest diagnostic: ", console)
    replay_start = diagnostic.start() if diagnostic else len(console)
    replay_marker = position(diagnostic.end()) if diagnostic else None
    lines, line_start = [], 0
    for raw_line in console.splitlines(keepends=True):
        line_end = line_start + len(raw_line)
        # Same CR normalization as the production serial line reader.
        line = raw_line.replace("\r", "").rstrip("\n")
        lines.append({**position(line_end), "startCharacter": line_start, "text": line,
                      "beforeDiagnosticReplay": line_start < replay_start})
        line_start = line_end
    finished_at = result["finishedAt"]

    phases = {
        "guest_boot": r"^LAB\|boot$",
        "executables_staged": r"^\[browser-guest\] executables staged;",
        "postgres_init": r"^\[browser-single\] initializing isolated PostgreSQL",
        "postgres_started": r"^server started$",
        "openfga_migrate_start": r"^\[browser-single\] migrating and starting real OpenFGA",
        "openfga_listener_wait": r"^\[browser-single\] waiting for the OpenFGA HTTP listener",
        "warmup_start": r"^\[browser-openfga\].* warmup_start ",
        "warmup_end": r"^\[browser-openfga\].* warmup_end ",
        "grpc_dial_start": r"^\[browser-openfga\].* grpc_dial_start ",
        "grpc_dial_end": r"^\[browser-openfga\].* grpc_dial_end ",
        "model_bind_start": r"^\[browser-single\] binding the pinned Lakekeeper model",
        "axum_start": r"^\[browser-single\] starting real Axum",
        "service_error": r"^Error:",
        "service_exited": r"^\[browser-single\] a required service exited",
        "ready": r"^LAB\|ready$",
    }
    first_phases, checks, topology_markers = {}, [], []
    last_original_service = None
    for line in lines:
        if not line["beforeDiagnosticReplay"]:
            continue
        text = line["text"]
        recognized_phase = False
        for name, pattern in phases.items():
            if re.search(pattern, text):
                recognized_phase = True
                if name not in first_phases:
                    first_phases[name] = {**line, "phase": name}
        if text.startswith("[browser-openfga]") and '/openfga.v1.OpenFGAService/Check"' in text:
            checks.append(line)
        if re.search(r"^\[browser-guest\] confirmed (?:online CPUs|service CPU availability):", text):
            topology_markers.append(line)
        # Keep last-progress classification aligned with the phase list. In
        # particular, pg_ctl's unprefixed "server started" is real progress.
        if recognized_phase or re.search(r"^(?:\[browser-(?:guest|single|openfga)\]|LAB\||Error:|Listening on)", text):
            last_original_service = line

    counter_keys = ["attempted_total", "module_created_total", "instance_created_total", "registered_total",
                    "detached_total", "gc_total", "encoded_bytes_total", "encoded_bytes_max"]
    cores, malformed = {}, []
    decoder = json.JSONDecoder()
    # A host print can arrive between partial guest serial chunks. Find the
    # actual telemetry prefix even when it follows unfinished guest text.
    for match in re.finditer(r"\[browser-qemu\] ", console):
        line = {**position(match.end()), "startCharacter": match.start(),
                "beforeDiagnosticReplay": match.start() < replay_start}
        try:
            value, consumed = decoder.raw_decode(console[match.end():match.end() + 16384])
            line.update(position(match.end() + consumed))
            assert isinstance(value["core_id"], int) and value["core_id"] >= 0
            assert all(type(value[key]) is int and value[key] >= 0 for key in counter_keys)
        except (ValueError, KeyError, AssertionError) as error:
            malformed.append({"streamLine": line["streamLine"], "error": str(error)})
            continue
        state = cores.setdefault(value["core_id"], {"original": [], "duplicateSnapshots": 0,
                                                    "regressiveOrReplayedSnapshots": [], "observedFrames": 0})
        state["observedFrames"] += 1
        previous = state["original"][-1] if state["original"] else None
        if previous and any(value[key] < previous[key] for key in counter_keys):
            state["regressiveOrReplayedSnapshots"].append({"streamLine": line["streamLine"],
                "receivedAt": line["receivedAt"], "beforeDiagnosticReplay": line["beforeDiagnosticReplay"],
                "registered_total": value["registered_total"], "gc_total": value["gc_total"]})
            continue
        if previous and all(value[key] == previous[key] for key in counter_keys) and value.get("event") == previous.get("event"):
            state["duplicateSnapshots"] += 1
            continue
        snapshot = {key: line[key] for key in ["receivedAt", "streamLine", "startCharacter", "beforeDiagnosticReplay"]}
        snapshot.update(value)
        snapshot["ageMsAtResultFinish"] = elapsed(snapshot["receivedAt"], finished_at)
        ordered = value["registered_total"] >= value["detached_total"] >= value["gc_total"]
        snapshot["perCoreCounterDifferences"] = {
            "registeredMinusDetached": value["registered_total"] - value["detached_total"],
            "detachedMinusGC": value["detached_total"] - value["gc_total"],
            "registeredMinusGC": value["registered_total"] - value["gc_total"],
            "registrationCountersConsistent": ordered,
            "scope": "Counts for this core at this snapshot only; not bytes or a contemporaneous global count.",
        }
        state["original"].append(snapshot)

    core_reports = []
    for core, state in sorted(cores.items()):
        samples = state.pop("original")
        if not samples:
            continue
        before = [sample for sample in samples if sample["beforeDiagnosticReplay"]]
        gc_intervals = sum(b["gc_total"] > a["gc_total"] for a, b in zip(samples, samples[1:]))
        gaps = [{"milliseconds": elapsed(a["receivedAt"], b["receivedAt"]),
                 "from": a["receivedAt"], "to": b["receivedAt"]} for a, b in zip(samples, samples[1:])]
        core_reports.append({"core": core, **state, "monotonicSnapshots": len(samples),
            "first": samples[0], "lastMonotonicSnapshot": samples[-1],
            "lastBeforeDiagnosticReplay": before[-1] if before else None,
            "postDiagnosticMonotonicSnapshots": len(samples) - len(before),
            "gcIncreaseIntervals": gc_intervals,
            "registrationDelta": samples[-1]["registered_total"] - samples[0]["registered_total"],
            "gcDelta": samples[-1]["gc_total"] - samples[0]["gc_total"],
            "largestSnapshotReceiptGaps": sorted(gaps, key=lambda gap: gap["milliseconds"], reverse=True)[:3]})
    source_gaps = [{"milliseconds": elapsed(a["receivedAt"], b["receivedAt"]),
                    "from": a["receivedAt"], "to": b["receivedAt"],
                    "fromStreamLine": a["streamLine"], "toStreamLine": b["streamLine"]}
                   for a, b in zip(nonempty, nonempty[1:])]
    return {
        "engine": engine, "browser": result.get("browser"), "resultStartedAt": result["startedAt"],
        "resultFinishedAt": finished_at, "logCapture": capture, "rawFiles": raw_records,
        "rawHashesMatchSummary": summary is not None, "fullStreamReconstructionVerified": True,
        "workflowCommit": summary.get("workflowCommit") if summary else None,
        "requestedVirtualCPUs": summary.get("requestedVirtualCPUs") if summary else None,
        "observedTopology": summary.get("observedTopology") if summary else None,
        "vm": result.get("guest", {}).get("vm"), "passed": result["passed"],
        "passes": result.get("passes"), "failure": result.get("failure"), "pageErrors": result.get("pageErrors"),
        "firstSourceReceivedAt": nonempty[0]["receivedAt"], "lastSourceReceivedAt": nonempty[-1]["receivedAt"],
        "finalSourceSilenceMs": elapsed(nonempty[-1]["receivedAt"], finished_at),
        "largestSourceReceiptGaps": sorted(source_gaps, key=lambda gap: gap["milliseconds"], reverse=True)[:8],
        "diagnosticReplayBegins": replay_marker,
        "phasesBeforeDiagnosticReplay": list(first_phases.values()), "checkRpcsBeforeDiagnosticReplay": checks,
        "topologyMarkersBeforeDiagnosticReplay": topology_markers,
        "lastOriginalServiceMarker": last_original_service,
        "lastServiceMarkerAgeMsAtResultFinish": elapsed(last_original_service["receivedAt"], finished_at) if last_original_service else None,
        "cores": core_reports, "malformedTelemetry": malformed,
        "interpretationLimits": [
            "All times are host batch receipt/completion times; they do not measure guest clock speed or prove a deadlock.",
            "Service/RPC markers stop before the first diagnostic tail replay; no replay is counted as a new RPC.",
            "Per-core telemetry discards duplicate/regressive vectors, labels monotonic post-diagnostic samples separately, and preserves pre-diagnostic endpoints.",
            "Core snapshots have different ages. Never sum them as a simultaneous global live count or infer budget saturation from stale sums.",
            "Counters count module lifecycle events; encoded byte totals are Wasm inputs, not native executable memory.",
            "Observed GC counter increments establish delivered callbacks only in the observed intervals, with no future FinalizationRegistry scheduling guarantee.",
            "Absence of snapshots does not prove no compilations: telemetry is sparse and may report only threshold crossings.",
        ],
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--engine", choices=["chromium", "firefox"], required=True)
    parser.add_argument("--output", type=Path)
    arguments = parser.parse_args()
    report = analyze(arguments.archive, arguments.engine)
    report["archive"] = {"path": str(arguments.archive), **record(arguments.archive.read_bytes())}
    text = json.dumps(report, indent=2, ensure_ascii=False) + "\n"
    if arguments.output:
        arguments.output.write_text(text)
        print(json.dumps({"output": str(arguments.output), "engine": report["engine"],
            "requestedVirtualCPUs": report["requestedVirtualCPUs"], "rawReconstructionVerified": True,
            "cores": len(report["cores"]), "passed": report["passed"]}))
    else:
        print(text, end="")
