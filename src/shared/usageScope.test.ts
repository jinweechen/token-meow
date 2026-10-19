import assert from "node:assert/strict";
import test from "node:test";
import { demoSnapshot } from "./demo";
import { demoUsageDashboard } from "./usageDashboardDemo";
import { usageCollectionCoverage, usageForecastBlockedReason, usageScopeQuery } from "./usageScope";
import type { Snapshot, SourceInstance } from "./contracts";

function checkedSource(id = "a"): SourceInstance {
  return { id, root: "", clientKind: "codex", enabled: true, runtime: "可用",
    health: { pendingFiles: 0, issues: 0, unsupportedFiles: 0,
      lastSuccessfulCheck: "2026-10-19T01:00:00Z" } };
}

function checkedSnapshot(): Snapshot {
  const snapshot = demoSnapshot();
  snapshot.health.lastSuccessfulScan = "2026-10-19T01:00:00Z";
  snapshot.health.pendingFiles = 0;
  snapshot.health.history = { status: "idle", pendingFiles: 0 };
  snapshot.health.issues = [];
  snapshot.health.unsupportedFiles = 0;
  snapshot.sources = [checkedSource()];
  return snapshot;
}

test("scope drill-down preserves the exact visible date/model/provider/root intersection", () => {
  const query = {
    from: "2026-10-10",
    to: "2026-10-16",
    model: "m",
    providerId: "p",
    rootTurnId: "root",
    agentKind: "subagent" as const,
  };
  const row = {
    projectId: "path:d:/project",
    sourceId: "source-b",
    threadId: "same-thread",
  };
  assert.deepEqual(usageScopeQuery(query, "sessions", row), {
    ...query,
    sourceId: "source-b",
    threadId: "same-thread",
  });
  assert.deepEqual(
    usageScopeQuery(query, "projects", { ...row, sourceId: null }),
    { ...query, project: row.projectId },
  );
  assert.deepEqual(query, {
    from: "2026-10-10",
    to: "2026-10-16",
    model: "m",
    providerId: "p",
    rootTurnId: "root",
    agentKind: "subagent",
  });
});

test("private or incomplete navigation identity cannot produce a scope", () => {
  assert.equal(
    usageScopeQuery({}, "sessions", {
      sourceId: null,
      threadId: "thread",
      projectId: null,
    }),
    null,
  );
  assert.equal(
    usageScopeQuery({}, "projects", {
      sourceId: null,
      threadId: null,
      projectId: null,
    }),
    null,
  );
});

test("collection, scope conflicts and relevant source failures block forecasting", () => {
  const snapshot = demoSnapshot();
  snapshot.health.lastSuccessfulScan = "2026-10-17T00:00:00Z";
  snapshot.health.history = { status: "idle", pendingFiles: 0 };
  snapshot.health.pendingFiles = 0;
  snapshot.sources = [
    {
      id: "a",
      clientKind: "codex",
      enabled: true,
      root: "",
      health: { issues: 1 },
    },
    {
      id: "b",
      clientKind: "codex",
      enabled: true,
      root: "",
      health: { issues: 0 },
    },
  ];
  assert.match(
    usageForecastBlockedReason(snapshot, { sourceId: "a" })!,
    /采集问题/,
  );
  assert.equal(usageForecastBlockedReason(snapshot, { sourceId: "b" }), null);
  snapshot.health.pendingFiles = 2;
  assert.match(
    usageForecastBlockedReason(snapshot, { sourceId: "b" })!,
    /导入尚未完成/,
  );
  snapshot.health.pendingFiles = 0;
  snapshot.health.history.status = "paused";
  assert.match(usageForecastBlockedReason(snapshot, {})!, /导入尚未完成/);
  snapshot.health.history.status = "idle";
  snapshot.health.lastSuccessfulScan = null;
  assert.match(usageForecastBlockedReason(snapshot, {})!, /尚无成功采集/);
});

test("partial imports expose known pending files without adding overlapping collector/history counts", () => {
  const snapshot = checkedSnapshot();
  snapshot.health.pendingFiles = 363;
  snapshot.health.history = { status: "running", pendingFiles: 363 };
  snapshot.sources![0].health!.pendingFiles = 363;
  const notice = usageCollectionCoverage(snapshot, {});
  assert.match(notice.title, /正在导入.*尚不完整/);
  assert.deepEqual(notice.notes, ["已知仍有 363 个文件待采集。"]);
  assert.equal(notice.attention, true);
  // Pending files remain an import condition even if history status lagged idle.
  snapshot.health.history.status = "idle";
  assert.match(usageCollectionCoverage(snapshot, {}).title, /正在导入/);
});

test("paused and cancelled history stay visible at zero pending without claiming live collection stopped", () => {
  for (const status of ["paused", "cancelled"] as const) {
    const snapshot = checkedSnapshot();
    snapshot.health.history!.status = status;
    const notice = usageCollectionCoverage(snapshot, {});
    assert.match(notice.title, status === "paused" ? /补导已暂停/ : /补导已取消/);
    assert.match(notice.notes.join(" "), /不会因此停止实时新增记录的采集/);
    assert.doesNotMatch(notice.title, /正在导入|完整/);
    assert.equal(notice.attention, true);
  }
});

test("missing or malformed collection state is not converted to known zero or complete coverage", () => {
  for (const value of [null, undefined, "0", -1, Number.NaN]) {
    const snapshot = checkedSnapshot();
    snapshot.health.pendingFiles = value;
    snapshot.health.history!.pendingFiles = null;
    const notice = usageCollectionCoverage(snapshot, {});
    assert.equal(notice.title, "采集覆盖状态尚未确认");
    assert.match(notice.notes.join(" "), /不能据此判断覆盖完整/);
  }
  const snapshot = checkedSnapshot();
  delete snapshot.health.history;
  assert.equal(usageCollectionCoverage(snapshot, {}).title, "采集覆盖状态尚未确认");
});

test("a successful global scan does not hide a selected source with no successful check", () => {
  const snapshot = checkedSnapshot();
  snapshot.sources![0].health!.lastSuccessfulCheck = null;
  const notice = usageCollectionCoverage(snapshot, { sourceId: "a" });
  assert.equal(notice.title, "等待成功采集检查");
  assert.match(notice.notes.join(" "), /尚无.*成功采集检查/);
  snapshot.sources = [checkedSource()];
  snapshot.health.lastSuccessfulScan = null;
  assert.equal(usageCollectionCoverage(snapshot, {}).title, "等待成功采集检查");
});

test("source and client filters isolate collection problems from unrelated global aggregates", () => {
  const snapshot = checkedSnapshot();
  const bad = snapshot.sources![0];
  bad.health!.pendingFiles = 363;
  bad.health!.issues = 1;
  bad.runtime = "不可用";
  snapshot.health.pendingFiles = 363;
  snapshot.health.history = { status: "running", pendingFiles: 363 };
  snapshot.health.issues = [{ kind: "source-a-problem" }];
  const other = checkedSource("b");
  other.clientKind = "other";
  snapshot.sources!.push(other);
  for (const query of [{ sourceId: "b" }, { clientKind: "other" }]) {
    const notice = usageCollectionCoverage(snapshot, query);
    assert.equal(notice.title, "统计仅含已采集记录");
    assert.deepEqual(notice.notes, []);
    assert.equal(notice.attention, false);
  }
  assert.match(usageCollectionCoverage(snapshot, { sourceId: "a" }).title, /正在导入/);
  assert.match(usageCollectionCoverage(snapshot, {}).notes.join(" "), /采集、解析/);
  snapshot.health.history.status = "paused";
  const paused = usageCollectionCoverage(snapshot, { sourceId: "b" });
  assert.equal(paused.title, "历史补导已暂停");
  assert.doesNotMatch(paused.notes.join(" "), /363|文件待采集/);
});

test("completed cursor work and source quality issues remain different collection facts", () => {
  const snapshot = checkedSnapshot();
  snapshot.health.issues = [{ code: "legacy_missing_baseline" }];
  const notice = usageCollectionCoverage(snapshot, {});
  assert.equal(notice.title, "部分记录需要检查");
  assert.match(notice.notes.join(" "), /部分记录可能未计入/);
  assert.doesNotMatch(notice.title, /正在导入/);
  snapshot.health.issues = [];
  snapshot.sources![0].health!.unsupportedFiles = 2;
  assert.match(usageCollectionCoverage(snapshot, {}).notes.join(" "), /不支持的文件/);
});

test("disabled or missing sources cannot masquerade as a fresh complete collection", () => {
  const snapshot = checkedSnapshot();
  snapshot.sources![0].enabled = false;
  snapshot.sources![0].health!.pendingFiles = 12;
  snapshot.health.pendingFiles = 12;
  snapshot.health.history = { status: "running", pendingFiles: null };
  const disabled = usageCollectionCoverage(snapshot, { sourceId: "a" });
  assert.equal(disabled.title, "所选范围包含已停用来源");
  assert.match(disabled.notes.join(" "), /停用.*此前已采集/);
  assert.doesNotMatch(disabled.notes.join(" "), /12/);
  assert.equal(usageCollectionCoverage(snapshot, { sourceId: "missing" }).title, "等待成功采集检查");
  snapshot.sources = [];
  assert.doesNotMatch(usageCollectionCoverage(snapshot, {}).title, /正在导入/);
  delete snapshot.sources;
  assert.match(usageCollectionCoverage(snapshot, { sourceId: "a" }).notes.join(" "), /状态尚未提供/);
});

test("collection notices do not borrow forecast exclusions or mutate snapshots and scope", () => {
  const snapshot = checkedSnapshot();
  const query = { sourceId: "a", from: "2026-10-01", to: "2026-10-19" };
  snapshot.usageDashboard = demoUsageDashboard(snapshot, query, { page: 0, pageSize: 20, quality: "all" });
  snapshot.usageDashboard.summary.excludedResponses = 1;
  const original = structuredClone(snapshot);
  assert.match(usageForecastBlockedReason(snapshot, query)!, /计量缺失或冲突/);
  assert.equal(usageCollectionCoverage(snapshot, query).title, "统计仅含已采集记录");
  assert.deepEqual(snapshot, original);
  assert.deepEqual(query, { sourceId: "a", from: "2026-10-01", to: "2026-10-19" });
});

test("unknown history and unsafe pending aggregation never produce an apparently finished collection", () => {
  const snapshot = checkedSnapshot();
  snapshot.health.history = { status: "unknown", pendingFiles: 0 } as unknown as Snapshot["health"]["history"];
  assert.equal(usageCollectionCoverage(snapshot, {}).title, "采集覆盖状态尚未确认");
  snapshot.health.history = { status: "idle", pendingFiles: 0 };
  snapshot.sources!.push(checkedSource("b"));
  for (const source of snapshot.sources!) source.health!.pendingFiles = Number.MAX_SAFE_INTEGER;
  const notice = usageCollectionCoverage(snapshot, { clientKind: "codex" });
  assert.match(notice.title, /正在导入.*尚不完整/);
  assert.match(notice.notes.join(" "), /不能据此判断覆盖完整/);
  assert.doesNotMatch(notice.notes.join(" "), /个文件/);
});
