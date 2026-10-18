"""External read-only own-ledger/metrics observer. Never launches or controls UI.

Start before start-resource-job.ps1, using its new --job JSONL path. Only the
selected isolated data directory is read; output is append-only new evidence.
"""
import argparse
import datetime as dt
import hashlib
import json
import math
import pathlib
import sqlite3
import time
import shared_json_read as shared_json

WORKSPACE = pathlib.Path(__file__).resolve().parent.parent
OUTPUT = (WORKSPACE / "output").resolve()
FILETIME_EPOCH = 116444736000000000
OBSERVER_REVISION = "windows-shared-json-r1"
SHARED_JSON_READER_SHA256 = hashlib.sha256(pathlib.Path(shared_json.__file__).read_bytes()).hexdigest().upper()


def own_output(value):
    path = pathlib.Path(value).resolve()
    if OUTPUT not in path.parents:
        raise ValueError("Use only isolated workspace output data and evidence")
    return path


def utc_ms(value):
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp() * 1000


def read_json(path, maximum=2 * 1024 * 1024):
    return shared_json.read_json(path, maximum)


class JobTail:
    def __init__(self, path):
        self.path, self.offset, self.pending, self.rows = path, 0, b"", []

    def read(self):
        if not self.path.exists():
            return self.rows
        size = self.path.stat().st_size
        if size > 128 * 1024 * 1024 or size < self.offset:
            raise ValueError("Job file exceeded its bound or was truncated")
        with self.path.open("rb") as reader:
            reader.seek(self.offset)
            data = reader.read(2 * 1024 * 1024)
        self.offset += len(data)
        self.pending += data
        while b"\n" in self.pending:
            line, self.pending = self.pending.split(b"\n", 1)
            if len(line) > 1024 * 1024 or len(self.rows) >= 80000:
                raise ValueError("Job record bound exceeded")
            if line.strip():
                self.rows.append(json.loads(line.decode("utf-8-sig")))
        if len(self.pending) > 1024 * 1024:
            raise ValueError("Incomplete Job record exceeded its bound")
        return self.rows


def current_metrics(data, identity):
    document = shared_json.read_json_document(data / "desktop-metrics.json")
    metrics = document.value
    created_ms = (identity["rootCreationFileTime"] - FILETIME_EPOCH) / 10000
    now_ms = time.time_ns() / 1000000
    if metrics.get("processId") != identity["rootProcessId"] or utc_ms(metrics["createdAt"]) < created_ms:
        raise ValueError("Metrics belong to an old/different process")
    modified_ms = document.modified_unix_ns / 1000000
    if now_ms - modified_ms > 40000 or modified_ms > now_ms + 1000 or utc_ms(metrics["createdAt"]) > now_ms + 1000:
        raise ValueError("Fresh metrics file for bound process required")
    return metrics, document


def inspect(data, source, identity, metrics):
    database = data / "ledger.sqlite3"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=0.2) as db:
        db.execute("PRAGMA query_only=ON")
        settings = {key: json.loads(value) for key, value in db.execute("SELECT key,value FROM settings")}
        sources = list(db.execute("SELECT id,root FROM source_instances WHERE enabled=1 LIMIT 65"))
    # Compare filesystem identity: Core persists Windows canonical \\?\ paths,
    # while the observer may receive an ordinary drive path for the same root.
    if len(sources) != 1 or not pathlib.Path(sources[0][1]).samefile(source) or settings.get("primarySourceId") != sources[0][0]:
        raise ValueError("Idle fixture requires exactly the selected enabled source")
    preferences = settings.get("preferences", {})
    pending = settings.get("historyPending:" + sources[0][0])
    runtime = metrics.get("runtime", {})
    checks = {
        "historyCheckedAndIdle": pending == 0 and type(pending) is int,
        "historyNotPausedOrCancelled": settings.get("historyPaused", False) is False and settings.get("historyCancelled", False) is False,
        "pendingFilesZero": settings.get("pendingFiles") == 0 and type(settings.get("pendingFiles")) is int,
        "collectorFresh": 0 <= time.time() * 1000 - utc_ms(metrics["collector"]["lastSuccessAt"]) <= 40000,
        "mainHidden": metrics.get("windows", {}).get("main", {}).get("visible") is False,
        "petVisible": metrics.get("windows", {}).get("pet", {}).get("visible") is True and metrics.get("windows", {}).get("pet", {}).get("minimized") is False,
        "ordinaryDesktopAvailable": all(runtime.get(key) is False for key in ("locked", "suspended", "displayOff", "disconnected", "powerSaver", "paused")),
        "configuredStatic": preferences.get("reducedMotion") is True,
        "notExiting": not metrics.get("shutdown"),
    }
    return {"checks": checks, "allConditions": all(checks.values()), "frontendReadyMs": metrics.get("frontendReadyMs", {}), "collector": metrics.get("collector"), "runtime": metrics.get("runtime"), "windows": metrics.get("windows"), "staticEvidence": "Explicit Core reducedMotion preference; actual renderer consumption requires the native setup observation."}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--data", required=True)
    parser.add_argument("--source-root", required=True)
    parser.add_argument("--job", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--ready-window", choices=("main", "pet"), default="main")
    args = parser.parse_args()
    data, source, job, output = map(own_output, (args.data, args.source_root, args.job, args.out))
    output.parent.mkdir(parents=True, exist_ok=True)
    # Record existence before process creation: first-run import is not warm start.
    database_preexisted = (data / "ledger.sqlite3").exists()
    begin_ns, begin_mono = time.time_ns(), time.perf_counter_ns()
    identity = None
    ready = None
    observations = []
    maximum_clock_skew = 0
    deadline = time.monotonic() + 30
    tail = JobTail(job)
    with output.open("x", encoding="utf-8") as writer:
        def emit(row):
            writer.write(json.dumps(row, ensure_ascii=False) + "\n")
            writer.flush()
        emit({"kind": "observer-start", "qualification": "runtime-preconditions-and-conservative-readiness-only", "databasePreexisted": database_preexisted, "monitorStartedUnixNs": begin_ns, "readyWindow": args.ready_window, "observerSha256": hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(), "observerRevision": OBSERVER_REVISION, "sharedJsonReaderSha256": SHARED_JSON_READER_SHA256})
        while True:
            rows = tail.read()
            if rows and identity is None:
                identity = rows[0]
                if identity.get("kind") != "start" or identity.get("isFixture") is not False or identity.get("requestedSeconds") != 600:
                    raise ValueError("Expected the real application's 600-second Job protocol")
                created_ns = (identity["rootCreationFileTime"] - FILETIME_EPOCH) * 100
                if identity.get("assignBeforeResume") is not True or identity.get("breakawayAllowed") is not False:
                    raise ValueError("Job containment prerequisites absent")
                finish_deadline = time.monotonic() + int(identity.get("warmupSeconds", 0)) + 645
                emit({"kind": "identity", "rootProcessId": identity["rootProcessId"], "rootCreationFileTime": identity["rootCreationFileTime"], "executableSha256": identity["executableSha256"]})
            if identity is None:
                if time.monotonic() >= deadline:
                    raise TimeoutError("No fresh Job start appeared within thirty seconds")
                time.sleep(0.025)
                continue
            wall_ns, mono_ns = time.time_ns(), time.perf_counter_ns()
            skew_ms = abs(wall_ns - (begin_ns + mono_ns - begin_mono)) / 1000000
            maximum_clock_skew = max(maximum_clock_skew, skew_ms)
            try:
                metrics, document = current_metrics(data, identity)
                # Include every shared-read attempt in the conservative readiness
                # upper bound. Never time a successful read at its earlier start.
                wall_ns, mono_ns = time.time_ns(), time.perf_counter_ns()
                skew_ms = abs(wall_ns - (begin_ns + mono_ns - begin_mono)) / 1000000
                maximum_clock_skew = max(maximum_clock_skew, skew_ms)
                app_ready = metrics.get("frontendReadyMs", {}).get(args.ready_window)
                if ready is None and isinstance(app_ready, (int, float)) and math.isfinite(app_ready) and app_ready >= 0:
                    raw_ms = (wall_ns - created_ns) / 1000000
                    correction_ms = 2 * maximum_clock_skew + 1
                    upper_ms = raw_ms + correction_ms
                    ready = {"kind": "readiness", "window": args.ready_window, "observedUpperMs": upper_ms, "rawWallElapsedMs": raw_ms, "clockAndRoundingCorrectionMs": correction_ms, "applicationInstantMs": app_ready, "databasePreexisted": database_preexisted, "observerBeganBeforeProcess": begin_ns <= created_ns, "clockSkewMs": skew_ms, "gate": "passed-conservative-warm-start" if database_preexisted and begin_ns <= created_ns and maximum_clock_skew <= 50 and 0 <= raw_ms and upper_ms <= 3000 else "not-qualified-or-threshold-exceeded"}
                    emit(ready)
                value = inspect(data, source, identity, metrics)
                observation = {"kind": "state", "unixMs": wall_ns / 1000000, "clockSkewMs": skew_ms, "metricsRead": {"attempts": document.attempts, "bytes": document.byte_count, "modifiedUnixNs": document.modified_unix_ns, "completedUnixNs": document.completed_unix_ns}, **value}
            except (OSError, sqlite3.Error, ValueError, KeyError, TypeError) as error:
                observation = {"kind": "state", "unixMs": wall_ns / 1000000, "clockSkewMs": skew_ms, "allConditions": False, "error": str(error)}
            emit(observation)
            observations.append(observation)
            if rows and rows[-1].get("kind") == "end":
                samples = [row for row in rows if row.get("kind") == "sample"]
                if len(samples) < 2:
                    raise ValueError("No complete resource measurement interval")
                first, last = utc_ms(samples[0]["capturedUtc"]), utc_ms(samples[-1]["capturedUtc"])
                prior = [row for row in observations if row["unixMs"] <= first]
                after = [row for row in observations if row["unixMs"] >= last]
                selected = ([prior[-1]] if prior else []) + [row for row in observations if first < row["unixMs"] < last] + ([after[0]] if after else [])
                gaps = [b["unixMs"] - a["unixMs"] for a, b in zip(selected, selected[1:])]
                completed = bool(prior and after and gaps and max(gaps) <= 2500 and rows[-1].get("completed") is True and rows[-1].get("rootExited") is False)
                conditions = completed and all(row["allConditions"] and row["clockSkewMs"] <= 50 for row in selected)
                readiness_passed = ready is not None and ready["gate"] == "passed-conservative-warm-start"
                qualified = conditions and readiness_passed
                emit({"kind": "end", "gate": "passed-runtime-preconditions-only" if qualified else "not-qualified", "readiness": ready, "readinessRequired": True, "readinessPassed": readiness_passed, "conditionsCoverJobInterval": completed, "conditionsPassed": conditions, "samples": len(selected), "maximumGapMs": max(gaps) if gaps else None, "jobResourceGateSeparate": True, "actualRendererConsumptionSeparate": True})
                return 0 if qualified else 2
            if time.monotonic() >= finish_deadline:
                emit({"kind": "end", "gate": "not-qualified", "error": "No complete Job end within its bounded observation window", "readiness": ready})
                return 2
            time.sleep(0.025 if ready is None and mono_ns - begin_mono < 10000000000 else 1)


if __name__ == "__main__":
    raise SystemExit(main())
