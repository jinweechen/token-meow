import assert from "node:assert/strict";
import test from "node:test";
import type { Query } from "./contracts";
import { demoSnapshot } from "./demo";
import {
  applyRootFilter,
  applySessionFilter,
  applySourceFilter,
  identityValue,
  sessionFilterLabel,
  sessionFilterOptions,
} from "./filters";
import { QueryLifecycle, queryScopeReady } from "./queryLifecycle";

const now = new Date("2026-10-16T02:00:00Z");
const range = { from: "2026-10-10", to: "2026-10-16", includeLegacy: false };
const sessionA = { sourceId: "source-a", id: "same-thread" };
const sessionB = { sourceId: "source-b", id: "same-thread" };

test("session selection uses source and thread identity, clears incompatible target filters and preserves explicit dates", () => {
  const previous: Query = {
    ...range,
    sourceId: "source-a",
    threadId: "old-thread",
    rootTurnId: "old-root",
    project: "path:d:/old",
    model: "old-model",
    providerId: "old-provider",
    clientKind: "codex",
  };
  const selected = applySessionFilter(previous, identityValue(sessionB));
  assert.deepEqual(selected, {
    ...range,
    sourceId: "source-b",
    threadId: "same-thread",
  });
  assert.deepEqual(applySourceFilter(selected, "source-a"), {
    ...range,
    sourceId: "source-a",
  });
  assert.deepEqual(
    applyRootFilter(selected, identityValue({ sourceId: "source-a", id: "root" })),
    { ...range, sourceId: "source-a", rootTurnId: "root" },
  );
  assert.deepEqual(applySessionFilter(selected, ""), {
    ...range,
    sourceId: "source-b",
    threadId: undefined,
  });
  assert.equal(applySessionFilter(previous, "same-thread"), previous);
  assert.equal(applySessionFilter(previous, '["","same-thread"]'), previous);
});

test("observed session options keep independent same ids and retain a private clearable selected option after empty results", () => {
  const session = demoSnapshot().sessions[0];
  const options = sessionFilterOptions(
    [
      { ...session, ...sessionA, title: "PRIVATE A d:/secret-a" },
      { ...session, ...sessionB, title: "PRIVATE B d:/secret-b" },
      { ...session, sourceId: undefined, title: "unverified source" },
    ],
    {},
  );
  assert.equal(options.length, 2);
  assert.notEqual(identityValue(options[0]), identityValue(options[1]));
  const sources = [{ id: "source-a" }, { id: "source-b" }];
  for (const [index, option] of options.entries()) {
    const label = sessionFilterLabel(option, index, true, sources);
    assert.ok(!label.includes("PRIVATE"));
    assert.ok(!label.includes("secret"));
    assert.ok(!label.includes("same-thread"));
  }
  const selected = { sourceId: "source-b", threadId: "same-thread" };
  const empty = sessionFilterOptions([], selected);
  assert.equal(empty.length, 1);
  assert.equal(identityValue(empty[0]), identityValue(sessionB));
  assert.equal(empty[0].selectedOnly, true);
  assert.equal(sessionFilterLabel(empty[0], 0, true, sources), "已选会话 · 来源 2");
  assert.equal(applySessionFilter(selected, "").threadId, undefined);
});

test("global session scope and later export queries share the current identity and fence an older source reply", () => {
  const lifecycle = new QueryLifecycle("", now);
  const old = lifecycle.begin(now);
  const oldAccepted = { revision: old.revision, query: { ...old.query } };
  lifecycle.update(
    (query) => applySessionFilter(query, identityValue(sessionB)),
    now,
  );
  assert.equal(queryScopeReady(oldAccepted, lifecycle.state()), false);
  const queryForSnapshot = lifecycle.begin(now);
  const queryForExport = lifecycle.state().query;
  assert.deepEqual(queryForSnapshot.query, queryForExport);
  assert.equal(queryForExport.sourceId, "source-b");
  assert.equal(queryForExport.threadId, "same-thread");
  assert.equal(lifecycle.isCurrent(old, now), false);
  lifecycle.update((query) => applySourceFilter(query, "source-a"), now);
  assert.equal(lifecycle.state().query.sourceId, "source-a");
  assert.equal(lifecycle.state().query.threadId, undefined);
  assert.equal(lifecycle.isCurrent(queryForSnapshot, now), false);
});
