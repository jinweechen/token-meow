import assert from "node:assert/strict";
import test from "node:test";
import type { UsageDashboardData, UsageDashboardScopeGroup } from "./contracts";
import { demoSnapshot, emptySnapshot } from "./demo";
import { demoUsageDashboard } from "./usageDashboardDemo";

const request = { page: 0, pageSize: 100, quality: "all" as const };

function assertScopeTotals(
  groups: UsageDashboardScopeGroup[],
  data: UsageDashboardData,
) {
  for (const field of [
    "totalTokens",
    "inputTokens",
    "outputTokens",
    "cachedInputTokens",
    "responseCount",
  ] as const)
    assert.equal(
      groups.reduce((total, group) => total + (group[field] ?? 0), 0),
      data.summary[field],
      field,
    );
  assert.equal(
    groups.reduce((total, group) => total + group.pricedResponses, 0),
    data.summary.pricedResponses,
  );
  assert.equal(
    groups.reduce(
      (total, group) =>
        total + Math.round(Number(group.estimatedCostUsd ?? 0) * 1e9),
      0,
    ),
    Math.round(Number(data.summary.estimatedCostUsd) * 1e9),
  );
}

test("scope preview groups partition the same trusted responses and price coverage as the summary", () => {
  const snapshot = demoSnapshot();
  const data = demoUsageDashboard(snapshot, {}, request);
  assertScopeTotals(data.projects, data);
  assertScopeTotals(data.sessions, data);
  assert.equal(data.projects.length, 3);
  assert.equal(data.sessions.length, 4);
  for (const groups of [data.projects, data.sessions])
    for (let index = 1; index < groups.length; index++)
      assert.ok(groups[index - 1].totalTokens >= groups[index].totalTokens);
  assert.equal(
    data.projects.reduce((total, group) => total + group.sessionCount, 0),
    data.sessions.length,
  );
  assert.ok(data.projects.every((group) => group.threadId === null));
  assert.ok(data.projects.every((group) => group.key !== group.projectId));
  assert.ok(data.sessions.every((group) => group.sessionCount === 1));
  for (const group of [...data.projects, ...data.sessions]) {
    assert.ok(group.firstEventTime);
    assert.ok(group.lastEventTime);
    assert.ok(group.firstEventTime <= group.lastEventTime);
    assert.equal(
      group.cacheHitRate,
      group.inputTokens ? group.cachedInputTokens! / group.inputTokens : null,
    );
    assert.equal(group.cacheCoverage, group.inputTokens ? 1 : null);
  }
  // The legacy demo snapshot's authored project totals are deliberately not
  // used to calculate the response-backed dashboard groups.
  snapshot.projects.forEach((project) => (project.totalTokens = 1));
  assert.deepEqual(
    demoUsageDashboard(snapshot, {}, request).projects,
    data.projects,
  );
});

test("scope preview filters rebuild every aggregate from the filtered rows", () => {
  const snapshot = demoSnapshot();
  const day = snapshot.daily[0].date;
  const session = snapshot.sessions[2];
  const data = demoUsageDashboard(
    snapshot,
    {
      from: day,
      to: day,
      project: session.project,
      sourceId: "demo-source",
      threadId: session.id,
      model: session.model!,
    },
    request,
  );
  assertScopeTotals(data.projects, data);
  assertScopeTotals(data.sessions, data);
  assert.equal(data.projects.length, 1);
  assert.equal(data.sessions.length, 1);
  assert.equal(data.sessions[0].threadId, session.id);
  assert.equal(data.projects[0].projectId, session.project);
  assert.equal(data.projects[0].sessionCount, 1);
  assert.equal(data.summary.estimatedCostUsd, null);
  assert.equal(data.sessions[0].estimatedCostUsd, null);
  assert.deepEqual(data.forecastBasis.daily, [
    {
      date: day,
      totalTokens: data.summary.totalTokens,
      responseCount: data.summary.responseCount,
    },
  ]);
  const noMatch = demoUsageDashboard(
    snapshot,
    { sourceId: "other-source" },
    request,
  );
  assert.deepEqual(noMatch.projects, []);
  assert.deepEqual(noMatch.sessions, []);
  assert.deepEqual(noMatch.forecastBasis.daily, []);
});

test("privacy retains scope grouping and totals while removing all navigation identities and names", () => {
  const snapshot = demoSnapshot();
  const open = demoUsageDashboard(snapshot, {}, request);
  snapshot.settings.privacyMode = true;
  const hidden = demoUsageDashboard(snapshot, {}, request);
  for (const scope of ["projects", "sessions"] as const) {
    assert.equal(hidden[scope].length, open[scope].length);
    assertScopeTotals(hidden[scope], hidden);
    assert.deepEqual(
      hidden[scope].map((group) => group.key),
      open[scope].map((group) => group.key),
    );
    assert.ok(
      hidden[scope].every(
        (group) =>
          group.sourceId === null &&
          group.threadId === null &&
          group.projectId === null &&
          /^(项目|会话) \d+$/.test(group.name),
      ),
    );
  }
  const serialized = JSON.stringify({
    projects: hidden.projects,
    sessions: hidden.sessions,
    forecastBasis: hidden.forecastBasis,
    rows: hidden.rows,
  });
  for (const project of snapshot.projects) {
    assert.ok(!serialized.includes(project.path));
    assert.ok(!serialized.includes(project.id));
    assert.ok(!serialized.includes(project.name));
  }
  for (const session of snapshot.sessions) {
    assert.ok(!serialized.includes(session.id));
    assert.ok(!serialized.includes(session.title));
  }
  assert.ok(!serialized.includes("demo-source"));
  assert.ok(
    hidden.rows.every(
      (row) =>
        row.sourceId === null &&
        row.threadId === null &&
        row.responseId === null,
    ),
  );
});

test("detail quality and page never alter scope totals or forecast samples", () => {
  const snapshot = demoSnapshot();
  const data = demoUsageDashboard(snapshot, {}, request);
  for (const quality of ["all", "priced", "unpriced"] as const) {
    const filtered = demoUsageDashboard(
      snapshot,
      {},
      {
        page: 2,
        pageSize: 20,
        quality,
      },
    );
    assert.deepEqual(filtered.projects, data.projects);
    assert.deepEqual(filtered.sessions, data.sessions);
    assert.deepEqual(filtered.forecastBasis, data.forecastBasis);
  }
  assert.equal(
    data.forecastBasis.daily.reduce((sum, day) => sum + day.totalTokens, 0),
    data.summary.totalTokens,
  );
  assert.equal(
    data.forecastBasis.daily.reduce((sum, day) => sum + day.responseCount, 0),
    data.summary.responseCount,
  );
  assert.ok(data.forecastBasis.daily.every((day) => day.responseCount > 0));
});

test("demo never infers completed forecast turns from lifecycle examples in the same thread", () => {
  const snapshot = demoSnapshot();
  assert.ok(snapshot.turns.some((turn) => turn.status === "completed"));
  const data = demoUsageDashboard(snapshot, {}, request);
  assert.ok(data.summary.responseCount > 0);
  assert.deepEqual(data.forecastBasis.completedTurns, []);
  assert.equal(data.forecastBasis.omittedIncompleteTurns, 0);
  const empty = demoUsageDashboard(emptySnapshot(), {}, request);
  assert.deepEqual(empty.projects, []);
  assert.deepEqual(empty.sessions, []);
  assert.deepEqual(empty.forecastBasis, {
    daily: [],
    completedTurns: [],
    omittedIncompleteTurns: 0,
  });
});
