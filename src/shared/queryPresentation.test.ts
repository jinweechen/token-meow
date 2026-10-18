import assert from "node:assert/strict";
import test from "node:test";
import type { Query } from "./contracts";
import {
  activeScopeFilterKeys,
  clearScopeFilters,
  parentScopeQuery,
  removeScopeFilter,
} from "./queryPresentation";

test("removing a chip preserves the visible date and remaining filter intersection", () => {
  const query: Query = Object.freeze({
    from: "2026-10-10",
    to: "2026-10-17",
    includeLegacy: false,
    project: "project-a",
    model: "model-a",
    sourceId: "source-a",
    threadId: "thread-a",
    providerId: "provider-a",
  });
  const next = removeScopeFilter(query, "project");
  assert.deepEqual(next, {
    from: "2026-10-10",
    to: "2026-10-17",
    includeLegacy: false,
    model: "model-a",
    sourceId: "source-a",
    threadId: "thread-a",
    providerId: "provider-a",
  });
  assert.notEqual(next, query);
  assert.equal(query.project, "project-a");
});

test("removing the source clears dependent thread and root identities while retaining other filters", () => {
  const query: Query = Object.freeze({
    from: "2026-10-17",
    to: "2026-10-17",
    sourceId: "source-a",
    threadId: "same-thread-id",
    rootTurnId: "same-root-id",
    project: "project-a",
    model: "model-a",
    clientKind: "codex",
    providerId: "provider-a",
    agentKind: "subagent",
    includeLegacy: true,
  });
  assert.deepEqual(removeScopeFilter(query, "sourceId"), {
    from: "2026-10-17",
    to: "2026-10-17",
    project: "project-a",
    model: "model-a",
    clientKind: "codex",
    providerId: "provider-a",
    agentKind: "subagent",
    includeLegacy: true,
  });
  assert.equal(query.sourceId, "source-a");
  assert.equal(query.threadId, "same-thread-id");
  assert.equal(query.rootTurnId, "same-root-id");
});

test("returning to a parent removes one detail scope without losing model or date selection", () => {
  const query: Query = Object.freeze({
    from: "2026-10-10",
    to: "2026-10-17",
    model: "model-a",
    sourceId: "source-a",
    providerId: "provider-a",
    project: "project-a",
    threadId: "thread-a",
    rootTurnId: "root-a",
  });
  const fromThread = parentScopeQuery(query)!;
  assert.equal(fromThread.threadId, undefined);
  assert.equal(fromThread.rootTurnId, "root-a");
  assert.equal(fromThread.project, "project-a");
  const fromRoot = parentScopeQuery(fromThread)!;
  assert.equal(fromRoot.rootTurnId, undefined);
  assert.equal(fromRoot.project, "project-a");
  const fromProject = parentScopeQuery(fromRoot)!;
  assert.deepEqual(fromProject, {
    from: "2026-10-10",
    to: "2026-10-17",
    model: "model-a",
    sourceId: "source-a",
    providerId: "provider-a",
  });
  assert.equal(parentScopeQuery(fromProject), null);
  assert.equal(query.threadId, "thread-a");
});

test("clearing scope retains date and explicit legacy visibility without mutating the original", () => {
  const query: Query = Object.freeze({
    from: "2026-10-10",
    to: "2026-10-17",
    includeLegacy: false,
    project: "project-a",
    threadId: "thread-a",
    sourceId: "source-a",
    rootTurnId: "root-a",
    model: "model-a",
    clientKind: "codex",
    providerId: "provider-a",
    agentKind: "unknown",
  });
  assert.deepEqual(clearScopeFilters(query), {
    from: "2026-10-10",
    to: "2026-10-17",
    includeLegacy: false,
  });
  assert.deepEqual(clearScopeFilters({ includeLegacy: true }), {
    includeLegacy: true,
  });
  assert.deepEqual(clearScopeFilters({}), {});
  assert.equal(query.threadId, "thread-a");
});

test("chips follow scope order and omit empty, malformed, date and legacy values", () => {
  assert.deepEqual(
    activeScopeFilterKeys({
      from: "2026-10-10",
      to: "2026-10-17",
      includeLegacy: true,
      agentKind: "unknown",
      providerId: "provider-a",
      clientKind: "codex",
      sourceId: "source-a",
      model: "model-a",
      rootTurnId: "root-a",
      threadId: "thread-a",
      project: "project-a",
    }),
    [
      "project",
      "threadId",
      "rootTurnId",
      "model",
      "sourceId",
      "clientKind",
      "providerId",
      "agentKind",
    ],
  );
  const malformed = {
    project: "   ",
    threadId: "",
    rootTurnId: null,
    model: undefined,
    sourceId: 1,
    clientKind: false,
    providerId: "\t",
    agentKind: "invalid-agent",
  } as unknown as Query;
  assert.deepEqual(activeScopeFilterKeys(malformed), []);
  assert.equal(parentScopeQuery(malformed), null);
});
