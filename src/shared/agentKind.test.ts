import assert from "node:assert/strict";
import test from "node:test";
import type { AgentKind, Query, Session } from "./contracts";
import { demoSnapshot } from "./demo";
import {
  applyAgentKindFilter,
  applySessionFilter,
  applySourceFilter,
  identityValue,
  sessionAgentKind,
} from "./filters";
import { QueryLifecycle, queryScopeReady } from "./queryLifecycle";

test("session labels require a classified agent kind and never infer primary from absent parent metadata", () => {
  const session = demoSnapshot().sessions[0];
  for (const parentThreadId of [null, "observed-parent"])
    assert.equal(sessionAgentKind({ ...session, agentKind: undefined, parentThreadId }), "unknown");
  assert.equal(sessionAgentKind({ ...session, agentKind: "future" as AgentKind }), "unknown");
  assert.equal(sessionAgentKind({ ...session, agentKind: "primary" }), "primary");
  assert.equal(sessionAgentKind({ ...session, agentKind: "subagent" }), "subagent");
});

test("global role changes fence old replies and remain part of later identity filters and export scope", () => {
  const now = new Date("2026-10-16T02:00:00Z");
  const scope = new QueryLifecycle("", now);
  const old = scope.begin(now);
  const accepted = { revision: old.revision, query: { ...old.query } };
  scope.update((query) => applyAgentKindFilter(query, "subagent"), now);
  assert.equal(scope.isCurrent(old, now), false);
  assert.equal(queryScopeReady(accepted, scope.state()), false);
  scope.update((query) => applySourceFilter(query, "source-b"), now);
  scope.update((query) => applySessionFilter(query, identityValue({ sourceId: "source-b", id: "same" })), now);
  assert.equal(scope.state().query.agentKind, "subagent");
  const selected = scope.begin(now);
  const queryForExport = scope.state().query;
  assert.deepEqual(queryForExport, selected.query);
  scope.update((query) => applyAgentKindFilter(query, "unknown"), now);
  assert.equal(scope.isCurrent(selected, now), false);
  const cleared = applyAgentKindFilter(queryForExport, "");
  assert.equal(cleared.agentKind, undefined);
  assert.equal(cleared.sourceId, "source-b");
  assert.equal(cleared.threadId, "same");
  assert.equal(applyAgentKindFilter(cleared, "invalid"), cleared);
});

test("illustrative role filters scope all displayed fact lists without filtering global activity or budgets", () => {
  const all = demoSnapshot();
  for (const agentKind of ["primary", "subagent", "unknown"] as const) {
    const query: Query = { agentKind };
    const selected = demoSnapshot(query);
    assert.ok(selected.sessions.length > 0);
    assert.ok(selected.sessions.every((session: Session) => session.agentKind === agentKind));
    const ids = new Set(selected.sessions.map((session) => session.id));
    for (const rows of [selected.turns, selected.tools, selected.inbox])
      assert.ok(rows.every((row) => ids.has(row.threadId)));
    assert.deepEqual(selected.activity, all.activity);
    assert.deepEqual(selected.budgets, all.budgets);
    assert.ok((selected.overview.totalTokens ?? 0) < all.overview.totalTokens!);
  }
});
