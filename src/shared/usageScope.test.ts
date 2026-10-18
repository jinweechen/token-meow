import assert from "node:assert/strict";
import test from "node:test";
import { demoSnapshot } from "./demo";
import { usageForecastBlockedReason, usageScopeQuery } from "./usageScope";

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
