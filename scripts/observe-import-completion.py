"""Read-only completion sidecar for the separate full-first-import Job protocol.

No UI, RPC, source writes, product launches, or third-party modules. The driver
owns process launch/sampling; this observer only verifies its immutable binding,
the private frozen input inventory, committed SQLite cursors, and safe metrics.
"""
import argparse
import ctypes
from ctypes import wintypes
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import stat
import time
import uuid
import shared_json_read as shared_json

WORKSPACE = Path(__file__).absolute().parent.parent
PERFORMANCE = WORKSPACE / "output" / "performance"
MAX_SECONDS = 3600
FRESH_SECONDS = 40
FILETIME_EPOCH = 116444736000000000
OBSERVER_REVISION = "windows-shared-json-r1"
SHARED_JSON_READER_SHA256 = hashlib.sha256(Path(shared_json.__file__).read_bytes()).hexdigest().upper()


def utc():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def parse_time(value):
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()


def no_reparse(path):
    path = Path(os.path.abspath(path))
    for part in reversed((path, *path.parents)):
        value = os.lstat(part)
        if stat.S_ISLNK(value.st_mode) or getattr(value, "st_file_attributes", 0) & 0x400:
            raise ValueError("reparse_or_symlink_rejected")
    return path


def private_path(path):
    path = Path(os.path.abspath(path))
    path.relative_to(PERFORMANCE)
    ancestor = path
    while not os.path.lexists(ancestor):
        ancestor = ancestor.parent
    no_reparse(ancestor)
    return path


def same_file(left, right):
    return os.path.samefile(no_reparse(left), no_reparse(right))


def digest_file(path, stop_path=None):
    digest = hashlib.sha256()
    with open(no_reparse(path), "rb", buffering=0) as stream:
        while block := stream.read(2 * 1024 * 1024):
            if stop_path is not None and Path(stop_path).exists():
                raise InterruptedError("observer-stop-requested")
            digest.update(block)
    return digest.hexdigest()


def read_json(path, limit=8 * 1024 * 1024):
    return shared_json.read_json(path, limit)


def write_new_json(path, value):
    path = private_path(path)
    data = (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    temporary = path.with_name(path.name + ".writing-" + uuid.uuid4().hex)
    with open(temporary, "xb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    # On the Windows acceptance host rename is atomic and fails if target exists.
    if os.path.lexists(path):
        raise FileExistsError(path)
    os.rename(temporary, path)


def open_readonly_db(data):
    path = no_reparse(Path(data) / "ledger.sqlite3")
    connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=0.2)
    connection.execute("PRAGMA query_only=ON")
    return connection


def read_settings(connection):
    return {key: json.loads(value) for key, value in
            connection.execute("SELECT key,value FROM settings")}


def fixed_plan(path):
    path = private_path(path)
    plan = read_json(path)
    expected = {"protocol": "full-first-import-v1", "maxSeconds": 3600,
                "sampleMilliseconds": 1000, "maximumSampleGapSeconds": 2.5,
                "privatePeakLimitMiB": 600, "requiredFreshCompleteIterations": 3}
    for key, value in expected.items():
        if plan.get(key) != value:
            raise ValueError("frozen_import_protocol_mismatch:" + key)
    for key in ("sourceRoot", "dataDirectory", "sourceManifestPath", "prepareReceiptPath",
                "observerDirectory"):
        private_path(plan[key])
    no_reparse(plan["executable"])
    if Path(plan["executable"]).stem.casefold().replace("_", "-") != "token-meow":
        raise ValueError("application_executable_required")
    return plan


def frozen_files(plan, hash_inputs=False, stop_path=None):
    if digest_file(plan["sourceManifestPath"]).casefold() != plan["expectedSourceManifestSha256"].casefold():
        raise ValueError("source_manifest_digest_changed")
    manifest = read_json(plan["sourceManifestPath"])
    entries = manifest.get("files", [])
    if (manifest.get("status") != "verified" or not entries or
            len(entries) != manifest.get("inventoryFileCount") or
            len(entries) != manifest.get("verifiedFileCount")):
        raise ValueError("complete_verified_frozen_inventory_required")
    root = no_reparse(plan["sourceRoot"])
    result = []
    names = set()
    for entry in entries:
        relative = Path(entry["relativePath"])
        if relative.is_absolute() or ".." in relative.parts or relative.parts[0] not in ("sessions", "archived_sessions"):
            raise ValueError("inventory_path_outside_log_roots")
        if relative.suffix.casefold() != ".jsonl":
            raise ValueError("non_jsonl_inventory_entry")
        target = no_reparse(root / relative)
        target.relative_to(root)
        length = entry["completePrefixBytes"]
        hashes = [entry.get(key) for key in ("completePrefixSha256", "copiedPrefixSha256", "sourceRereadPrefixSha256")]
        if (entry.get("prefixVerified") is not True or not isinstance(length, int) or length < 0 or
                target.stat().st_size != length or len(set(hashes)) != 1 or not hashes[0]):
            raise ValueError("frozen_file_size_or_verification_invalid")
        if hash_inputs and digest_file(target, stop_path).casefold() != hashes[0].casefold():
            raise ValueError("frozen_input_file_digest_changed")
        key = os.path.normcase(str(target))
        if key in names:
            raise ValueError("duplicate_frozen_inventory_path")
        names.add(key)
        result.append((target, length))
    if sum(length for _, length in result) != manifest.get("copiedCompletePrefixBytes"):
        raise ValueError("frozen_inventory_byte_total_mismatch")
    discovered = set()
    for folder_name in ("sessions", "archived_sessions"):
        folder = root / folder_name
        if not os.path.lexists(folder):
            continue
        pending = [no_reparse(folder)]
        while pending:
            current = pending.pop()
            with os.scandir(current) as children:
                for child in children:
                    value = child.stat(follow_symlinks=False)
                    if stat.S_ISLNK(value.st_mode) or getattr(value, "st_file_attributes", 0) & 0x400:
                        raise ValueError("frozen_input_tree_contains_reparse")
                    if stat.S_ISDIR(value.st_mode):
                        pending.append(Path(child.path))
                    elif child.name.casefold().endswith(".jsonl"):
                        discovered.add(os.path.normcase(str(Path(child.path))))
    if discovered != names:
        raise ValueError("actual_frozen_log_tree_differs_from_manifest")
    return result


def selected_source(connection, plan):
    sources = connection.execute("SELECT id,root,client_kind FROM source_instances WHERE enabled=1").fetchall()
    if len(sources) != 1 or sources[0][2] != "codex" or not same_file(sources[0][1], plan["sourceRoot"]):
        raise ValueError("exactly_selected_enabled_source_required")
    settings = read_settings(connection)
    if settings.get("primarySourceId") != sources[0][0]:
        raise ValueError("primary_source_does_not_match_enabled_source")
    return sources[0][0], settings


def preflight(plan):
    receipt = read_json(plan["prepareReceiptPath"])
    safety = receipt.get("safety", {})
    if (receipt.get("kind") != "production-core-prepare-only" or receipt.get("completed") is not True or
            receipt.get("scanCalls") != 0 or receipt.get("snapshotCalls") != 0 or
            safety.get("dataDirectoryCreatedNew") is not True or
            safety.get("initialDataDirectoryEmpty") is not True or
            not same_file(receipt["dataDirectory"], plan["dataDirectory"]) or
            not same_file(receipt["sourceDirectory"], plan["sourceRoot"])):
        raise ValueError("fresh_scan_zero_prepare_only_receipt_required")
    if digest_file(plan["executable"]).casefold() != plan["expectedSha256"].casefold():
        raise ValueError("executable_digest_mismatch")
    files = frozen_files(plan, hash_inputs=True)
    connection = open_readonly_db(plan["dataDirectory"])
    try:
        source_id, settings = selected_source(connection, plan)
        for table in ("source_files", "ingested_records", "usage_responses", "legacy_usage_segments",
                      "turns", "items", "inbox_events", "fact_observations", "ingestion_issues",
                      "usage_aggregates", "context_snapshots", "timing_observations", "fact_aliases",
                      "excluded_facts", "account_snapshots", "account_usage_snapshots"):
            if connection.execute("SELECT COUNT(*) FROM " + table).fetchone()[0] != 0:
                raise ValueError("prepared_database_already_contains_collected_facts")
        if settings.get("historyPaused", False) or settings.get("historyCancelled", False):
            raise ValueError("prepared_history_must_not_be_paused_or_cancelled")
    finally:
        connection.close()
    return {"kind": "import-preflight", "passed": True, "observedAt": utc(),
            "observerRevision": OBSERVER_REVISION, "sharedJsonReaderSha256": SHARED_JSON_READER_SHA256,
            "sourceId": source_id, "inputFiles": len(files), "inputBytes": sum(n for _, n in files),
            "importSince": settings.get("importSince"), "clearWatermark": settings.get("clearWatermark"),
            "prepareReceiptSha256": digest_file(plan["prepareReceiptPath"]),
            "sourceManifestSha256": digest_file(plan["sourceManifestPath"]),
            "executableSha256": digest_file(plan["executable"]),
            "allFrozenInputHashesVerifiedBeforeLaunch": True, "applicationLaunched": False}


def running_identity(control, executable):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(ctypes.c_uint64)] * 4
    kernel.GetProcessTimes.restype = wintypes.BOOL
    kernel.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR,
                                                ctypes.POINTER(wintypes.DWORD)]
    kernel.QueryFullProcessImageNameW.restype = wintypes.BOOL
    kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    kernel.GetExitCodeProcess.restype = wintypes.BOOL
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.OpenProcess(0x1000, False, int(control["rootProcessId"]))
    if not handle:
        raise ValueError("bound_process_unavailable")
    try:
        values = [ctypes.c_uint64() for _ in range(4)]
        if not kernel.GetProcessTimes(handle, *[ctypes.byref(value) for value in values]):
            raise ctypes.WinError(ctypes.get_last_error())
        buffer = ctypes.create_unicode_buffer(32768)
        size = wintypes.DWORD(len(buffer))
        if not kernel.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(size)):
            raise ctypes.WinError(ctypes.get_last_error())
        exit_code = wintypes.DWORD()
        if not kernel.GetExitCodeProcess(handle, ctypes.byref(exit_code)):
            raise ctypes.WinError(ctypes.get_last_error())
        if values[0].value != control["rootCreationFileTime"] or not same_file(buffer.value, executable):
            raise ValueError("running_process_creation_or_image_mismatch")
        if exit_code.value != 259:
            raise ValueError("bound_process_exited")
    finally:
        kernel.CloseHandle(handle)


def canonical(path):
    value = str(path)
    if value.startswith("\\\\?\\"):
        value = value[4:]
    return os.path.normcase(os.path.abspath(value))


def probe(plan, control, files):
    metrics_path = Path(plan["dataDirectory"]) / "desktop-metrics.json"
    document = shared_json.read_json_document(metrics_path, 8 * 1024 * 1024)
    metrics = document.value
    now = time.time()
    metrics_modified = document.modified_unix_ns / 1_000_000_000
    created = (control["rootCreationFileTime"] - FILETIME_EPOCH) / 10_000_000
    collector = metrics.get("collector", {})
    success = parse_time(collector["lastSuccessAt"])
    metrics_created = parse_time(metrics["createdAt"])
    if (metrics.get("processId") != control["rootProcessId"] or metrics_created < created or metrics_created > now + 1 or
            now - metrics_modified > FRESH_SECONDS or now - success > FRESH_SECONDS or
            metrics_modified > now + 1 or success > now + 1):
        raise ValueError("fresh_metrics_for_bound_process_required")
    expected = {canonical(path): length for path, length in files}
    connection = open_readonly_db(plan["dataDirectory"])
    try:
        # One read transaction gives all counters the same committed SQLite view.
        connection.execute("BEGIN")
        source_id, settings = selected_source(connection, plan)
        if (source_id != control["sourceId"] or settings.get("importSince") != control.get("importSince") or
                settings.get("clearWatermark") != control.get("clearWatermark")):
            raise ValueError("source_or_import_scope_changed_after_preflight")
        paused = settings.get("historyPaused", False)
        cancelled = settings.get("historyCancelled", False)
        rows = connection.execute("SELECT path,offset,generation,context FROM source_files WHERE source_id=?",
                                  (source_id,)).fetchall()
        missing = dict(expected)
        not_at_end = 0
        processed = 0
        extra_files = 0
        for path, offset, generation, context_string in rows:
            key = canonical(path)
            length = missing.pop(key, None)
            if length is None:
                extra_files += 1
                continue
            context = json.loads(context_string)
            live = context.get("live_cursor")
            at_end = (offset == length and generation == 0 and context.get("caught_up") is True and
                      context.get("discard_offset", 0) == 0 and
                      (live is None or (live.get("offset") == length and
                       live.get("context", {}).get("discard_offset", 0) == 0 and
                       live.get("bootstrap_end") is None)))
            processed += min(max(offset, 0), length)
            not_at_end += not at_end
        issues = connection.execute("SELECT kind,COUNT(*) FROM ingestion_issues GROUP BY kind").fetchall()
        source_check = settings.get("lastSourceCheck:" + source_id, {})
        history_pending = settings.get("historyPending:" + source_id)
        pending = settings.get("pendingFiles")
    finally:
        connection.close()
    # Verify input lengths still match the frozen inventory; paths stay private.
    for path, length in files:
        if no_reparse(path).stat().st_size != length:
            raise ValueError("frozen_input_size_changed_during_import")
    complete = (not paused and not cancelled and not missing and not_at_end == 0 and extra_files == 0 and
                history_pending == 0 and pending == 0 and collector.get("lastReport", {}).get("pendingFiles") == 0)
    coverage = (not issues and source_check.get("issues") == 0 and source_check.get("unsupportedFiles") == 0)
    runtime = metrics.get("runtime", {})
    runtime_keys = ("locked", "suspended", "displayOff", "disconnected", "powerSaver", "paused")
    ordinary = all(runtime[key] is False for key in runtime_keys) if all(key in runtime for key in runtime_keys) else None
    return {"kind": "import-state", "observedAt": utc(), "rootProcessId": control["rootProcessId"],
            "metricsRead": {"attempts": document.attempts, "bytes": document.byte_count, "modifiedUnixNs": document.modified_unix_ns, "completedUnixNs": document.completed_unix_ns},
            "rootCreationFileTime": control["rootCreationFileTime"], "jobId": control["jobId"],
            "sourceId": source_id, "inputFiles": len(files), "inputBytes": sum(expected.values()),
            "registeredFiles": len(rows), "missingFiles": len(missing), "filesNotAtFrozenEnd": not_at_end,
            "unexpectedFiles": extra_files, "historicalBytesCommitted": processed,
            "historyPendingFiles": history_pending, "pendingFiles": pending,
            "paused": paused, "cancelled": cancelled,
            "collectorIteration": collector.get("iterations"), "collectorSuccessAt": collector["lastSuccessAt"],
            "allFrozenCursorsAtEnd": complete, "coverageWithoutKnownSkipOrUnsupported": coverage,
            "issuesByKind": dict(issues), "unsupportedFiles": source_check.get("unsupportedFiles"),
            "configuredStatic": settings.get("preferences", {}).get("reducedMotion") is True,
            "ordinaryRuntime": ordinary,
            "mainHidden": not metrics["windows"]["main"]["visible"] if "visible" in metrics.get("windows", {}).get("main", {}) else None,
            "petVisible": metrics.get("windows", {}).get("pet", {}).get("visible"),
            "rendererConsumptionEvidence": "Core reducedMotion configuration only; native renderer observation remains separate"}


class CompletionFence:
    def __init__(self):
        self.iteration = None
        self.success = None
        self.count = 0
        self.baseline = False

    def accept(self, state):
        iteration = state.get("collectorIteration")
        success = state.get("collectorSuccessAt")
        fresh = (isinstance(iteration, int) and iteration > 0 and isinstance(success, str) and
                 (self.iteration is None or iteration > self.iteration) and
                 (self.success is None or parse_time(success) > parse_time(self.success)))
        if not state.get("allFrozenCursorsAtEnd"):
            self.count = 0
            self.baseline = False
        elif fresh:
            if self.baseline:
                self.count += 1
            else:
                self.baseline = True
        if fresh:
            self.iteration, self.success = iteration, success
        return self.count >= 3


def observe(plan, control_path):
    directory = private_path(plan["observerDirectory"])
    result_path = directory / "completion.json"
    fence = CompletionFence()
    deadline = time.monotonic() + MAX_SECONDS + 10
    errors = []
    control = None
    outcome = None
    files = frozen_files(plan)
    with open(directory / "import-state.jsonl", "x", encoding="utf-8", buffering=1) as evidence:
        while time.monotonic() < deadline:
            if (directory / "stop.json").exists():
                break
            try:
                if control is None:
                    if not Path(control_path).exists():
                        time.sleep(0.1)
                        continue
                    control = read_json(control_path)
                    if (control.get("protocol") != "full-first-import-v1" or
                            control.get("executableSha256", "").casefold() != plan["expectedSha256"].casefold() or
                            control.get("sourceManifestSha256", "").casefold() != plan["expectedSourceManifestSha256"].casefold() or
                            not control.get("jobId") or control.get("rootProcessId", 0) <= 0 or
                            control.get("rootCreationFileTime", 0) <= FILETIME_EPOCH):
                        raise ValueError("unexpected_launch_protocol")
                running_identity(control, plan["executable"])
                state = probe(plan, control, files)
                terminal = fence.accept(state)
                state["freshCompleteIterations"] = fence.count
                evidence.write(json.dumps(state) + "\n")
                if terminal:
                    # Keep the parent Job sampling while every frozen input is
                    # hashed again. This conservative interval is included in
                    # import resource evidence; same-size mutations cannot pass.
                    frozen_files(plan, hash_inputs=True, stop_path=directory / "stop.json")
                    running_identity(control, plan["executable"])
                    state = probe(plan, control, files)
                    if not state["allFrozenCursorsAtEnd"]:
                        raise ValueError("completion_regressed_during_input_rehash")
                    valid = state["coverageWithoutKnownSkipOrUnsupported"] and not errors
                    outcome = {"kind": "import-completion", "protocol": "full-first-import-v1",
                               "completionQualified": valid, "scanReachedFrozenEnd": True,
                               "status": "complete" if valid else "complete-with-coverage-or-observation-gaps",
                               "freshCompleteIterations": fence.count, "lastState": state, "errors": errors,
                               "allFrozenInputHashesVerifiedAfterCompletion": True,
                               "observedAt": utc(), "jobId": control["jobId"],
                               "rootProcessId": control["rootProcessId"],
                               "rootCreationFileTime": control["rootCreationFileTime"]}
                    break
            except InterruptedError:
                break
            except (FileNotFoundError, KeyError) as failure:
                # Initial metrics may not exist yet. After a collector has been
                # observed, missing or incomplete fields are retained as a gap.
                if fence.iteration is not None:
                    errors.append(type(failure).__name__)
                evidence.write(json.dumps({"kind": "import-wait", "observedAt": utc(),
                                           "reason": type(failure).__name__}) + "\n")
                fence.count = 0
            except (OSError, ValueError, sqlite3.Error) as failure:
                reason = type(failure).__name__ + ":" + str(failure)
                errors.append(reason)
                evidence.write(json.dumps({"kind": "import-observation-error", "observedAt": utc(),
                                           "reason": reason}) + "\n")
                outcome = {"kind": "import-completion", "protocol": "full-first-import-v1",
                           "status": "observation-error", "completionQualified": False,
                           "scanReachedFrozenEnd": False, "errors": errors, "observedAt": utc()}
                break
            time.sleep(0.5)
    if outcome is None:
        outcome = {"kind": "import-completion", "protocol": "full-first-import-v1",
                   "status": "stopped-or-timeout", "completionQualified": False,
                   "scanReachedFrozenEnd": False, "errors": errors, "observedAt": utc()}
    if control is not None:
        outcome.update({"jobId": control["jobId"], "rootProcessId": control["rootProcessId"],
                        "rootCreationFileTime": control["rootCreationFileTime"]})
    outcome.update({"observerRevision": OBSERVER_REVISION, "sharedJsonReaderSha256": SHARED_JSON_READER_SHA256})
    write_new_json(result_path, outcome)


def self_test():
    directory = private_path(PERFORMANCE / ("import-helper-selftest-" + uuid.uuid4().hex))
    directory.mkdir()
    cases = []
    def state(iteration, complete=True):
        return {"collectorIteration": iteration, "collectorSuccessAt": f"2026-10-15T12:00:{iteration:02d}+00:00",
                "allFrozenCursorsAtEnd": complete}
    fence = CompletionFence()
    assert not fence.accept(state(1, False)); cases.append("incomplete_cursors_never_complete")
    assert not fence.accept(state(2)); assert not fence.accept(state(2));
    assert fence.count == 0; cases.append("same_iteration_not_counted_twice")
    assert not fence.accept(state(3)); assert not fence.accept(state(4)); assert fence.accept(state(5))
    cases.append("three_fresh_iterations_after_completion_baseline_required")
    assert not fence.accept(state(6, False)); assert fence.count == 0; cases.append("regression_resets_completion_fence")
    malformed = CompletionFence(); assert not malformed.accept({"allFrozenCursorsAtEnd": True})
    cases.append("missing_collector_cannot_complete")
    target = directory / "identity"
    target.mkdir()
    assert same_file(str(target), str(target / ".")); cases.append("directory_identity_uses_samefile")
    # Exercise the production probe against only new private fixture SQLite/files.
    source = directory / "source"
    (source / "sessions").mkdir(parents=True)
    log = source / "sessions" / "fixture.jsonl"
    log.write_bytes(b'{}\n')
    data = directory / "data"
    data.mkdir()
    connection = sqlite3.connect(data / "ledger.sqlite3")
    connection.executescript("CREATE TABLE source_instances(id TEXT,root TEXT,enabled INTEGER,client_kind TEXT);"
                             "CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT);"
                             "CREATE TABLE source_files(path TEXT,offset INTEGER,generation INTEGER,context TEXT,source_id TEXT);"
                             "CREATE TABLE ingestion_issues(kind TEXT);")
    connection.execute("INSERT INTO source_instances VALUES('fixture-source',?,1,'codex')", (str(source),))
    settings = {"primarySourceId": "fixture-source", "pendingFiles": 0,
                "historyPending:fixture-source": 0,
                "lastSourceCheck:fixture-source": {"issues": 0, "unsupportedFiles": 0}}
    connection.executemany("INSERT INTO settings VALUES(?,?)", [(key, json.dumps(value)) for key, value in settings.items()])
    context = {"caught_up": True, "live_cursor": {"offset": 3, "context": {}}}
    connection.execute("INSERT INTO source_files VALUES(?,3,0,?,'fixture-source')", (str(log), json.dumps(context)))
    connection.commit()
    metrics = {"processId": 99999, "createdAt": utc(), "collector": {"iterations": 1,
               "lastSuccessAt": utc(), "lastReport": {"pendingFiles": 0}}}
    write_new_json(data / "desktop-metrics.json", metrics)
    control = {"rootProcessId": 99999, "rootCreationFileTime": int((time.time() - 1) * 10_000_000) + FILETIME_EPOCH,
               "sourceId": "fixture-source", "jobId": "protocol-fixture"}
    plan = {"sourceRoot": str(source), "dataDirectory": str(data)}
    assert probe(plan, control, [(log, 3)])["allFrozenCursorsAtEnd"]
    cases.append("sqlite_exact_frozen_cursor_and_zero_pending")
    connection.execute("UPDATE source_files SET offset=2"); connection.commit()
    assert not probe(plan, control, [(log, 3)])["allFrozenCursorsAtEnd"]
    cases.append("sqlite_partial_cursor_cannot_complete")
    connection.execute("UPDATE source_files SET offset=3")
    connection.execute("UPDATE settings SET value='1' WHERE key='pendingFiles'"); connection.commit()
    assert not probe(plan, control, [(log, 3)])["allFrozenCursorsAtEnd"]
    cases.append("sqlite_nonzero_pending_cannot_complete")
    connection.execute("UPDATE settings SET value='0' WHERE key='pendingFiles'")
    connection.execute("INSERT INTO ingestion_issues VALUES('record_too_large')"); connection.commit()
    assert not probe(plan, control, [(log, 3)])["coverageWithoutKnownSkipOrUnsupported"]
    cases.append("sqlite_skipped_record_is_not_coverage_complete")
    connection.execute("DELETE FROM settings WHERE key='primarySourceId'"); connection.commit()
    rejected = False
    try:
        probe(plan, control, [(log, 3)])
    except ValueError:
        rejected = True
    assert rejected; cases.append("sqlite_missing_primary_identity_rejected")
    connection.close()
    stop_file = directory / "stop-fixture.json"
    write_new_json(stop_file, {"stop": True})
    stopped = False
    try:
        digest_file(log, stop_file)
    except InterruptedError:
        stopped = True
    assert stopped; cases.append("rehash_honors_observer_stop_without_touching_application")
    write_new_json(directory / "receipt.json", {"qualification": "observer-protocol-self-test-only",
        "passed": True, "cases": cases, "applicationLaunched": False, "productPerformanceQualified": False})
    print(json.dumps({"passed": True, "cases": len(cases), "outputRoot": str(directory), "applicationLaunched": False}))


def main():
    parser = argparse.ArgumentParser()
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--preflight", action="store_true")
    group.add_argument("--observe", action="store_true")
    group.add_argument("--self-test", action="store_true")
    parser.add_argument("--plan")
    parser.add_argument("--control")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    plan = fixed_plan(args.plan)
    if args.preflight:
        print(json.dumps(preflight(plan)))
    else:
        observe(plan, args.control)


if __name__ == "__main__":
    main()
