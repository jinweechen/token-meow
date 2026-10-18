import assert from "node:assert/strict";
import test from "node:test";
import {
  QueryLifecycle,
  queryScopeReady,
  type DatePeriod,
  type QueryState,
} from "./queryLifecycle";
import { ViewRefreshLoop, type IntervalHost } from "./viewRefreshLoop";
import { changesQueryDomain, isQueryDomainChange } from "./queryDomain";

const beforeMidnight = new Date("2026-10-15T15:59:59.900Z");
const afterMidnight = new Date("2026-10-15T16:00:00.100Z");

test("pet inbox navigation atomically replaces every old filter with today's all-source nonlegacy query", () => {
  const scope = new QueryLifecycle("", beforeMidnight);
  scope.selectPeriod("custom", beforeMidnight);
  scope.update({
    from: "2026-09-01", to: "2026-09-30", sourceId: "secondary",
    threadId: "thread", rootTurnId: "root", agentKind: "subagent",
    project: "private/project", model: "other-model", providerId: "provider",
    clientKind: "other-client", includeLegacy: true,
  }, beforeMidnight);
  const old = scope.begin(beforeMidnight);
  assert.equal(scope.openPetInbox("2026-10-15", beforeMidnight), true);
  assert.equal(scope.state().period, "1");
  assert.deepEqual(scope.state().query, { from: "2026-10-15", to: "2026-10-15", includeLegacy: false });
  assert.equal(scope.isCurrent(old, beforeMidnight), false);
  const sameRange = scope.begin(beforeMidnight);
  assert.equal(scope.openPetInbox("2026-10-15", beforeMidnight), true);
  assert.equal(scope.isCurrent(sameRange, beforeMidnight), false);
});

test("expired pet inbox day is rejected without modifying custom query or in-flight authority", () => {
  const scope = new QueryLifecycle("", afterMidnight);
  scope.selectPeriod("custom", afterMidnight);
  scope.update({ from: "2026-09-02", to: "2026-09-03", sourceId: "kept" }, afterMidnight);
  const before = scope.state(), ticket = scope.begin(afterMidnight);
  for (const day of ["2026-10-15", "2026-10-17", "", "2026-1-16"])
    assert.equal(scope.openPetInbox(day, afterMidnight), false);
  assert.deepEqual(scope.state(), before);
  assert.equal(scope.isCurrent(ticket, afterMidnight), true);
});

test("query lifecycle defaults pair the pet today and main seven-day period with their Shanghai ranges", () => {
  const pet = new QueryLifecycle("?window=pet", beforeMidnight).state();
  assert.equal(pet.period, "1");
  assert.deepEqual(pet.query, {
    from: "2026-10-15",
    to: "2026-10-15",
    includeLegacy: false,
  });
  const main = new QueryLifecycle("", beforeMidnight).state();
  assert.equal(main.period, "7");
  assert.deepEqual(main.query, {
    from: "2026-10-09",
    to: "2026-10-15",
    includeLegacy: false,
  });
});

test("automatic presets follow forward midnight and clock rollback without losing filters", () => {
  const starts: [DatePeriod, string, string][] = [
    ["1", "2026-10-15", "2026-10-16"],
    ["7", "2026-10-09", "2026-10-10"],
    ["30", "2026-09-16", "2026-09-17"],
  ];
  for (const [period, oldFrom, nextFrom] of starts) {
    const lifecycle = new QueryLifecycle("", beforeMidnight);
    lifecycle.selectPeriod(period, beforeMidnight);
    lifecycle.update(
      (query) => ({ ...query, sourceId: "source-a", threadId: "thread-a" }),
      beforeMidnight,
    );
    const old = lifecycle.begin(beforeMidnight);
    assert.equal(old.query.from, oldFrom);
    assert.equal(lifecycle.synchronize(afterMidnight), true);
    assert.equal(lifecycle.state().query.from, nextFrom);
    assert.equal(lifecycle.state().query.to, "2026-10-16");
    assert.equal(lifecycle.state().query.sourceId, "source-a");
    assert.equal(lifecycle.state().query.threadId, "thread-a");
    assert.equal(lifecycle.isCurrent(old, afterMidnight), false);
    const next = lifecycle.begin(afterMidnight);
    assert.equal(lifecycle.synchronize(beforeMidnight), true);
    assert.equal(lifecycle.state().query.from, oldFrom);
    assert.equal(lifecycle.state().query.to, "2026-10-15");
    assert.equal(lifecycle.isCurrent(next, beforeMidnight), false);
  }
});

test("custom dates stay explicit across resume and rollback, then a selected preset returns to the current day", () => {
  const lifecycle = new QueryLifecycle("", beforeMidnight);
  lifecycle.selectPeriod("custom", beforeMidnight);
  lifecycle.update(
    (query) => ({ ...query, from: "2026-09-02", to: "2026-09-03" }),
    beforeMidnight,
  );
  const custom = lifecycle.state().query;
  assert.equal(lifecycle.synchronize(new Date("2026-11-01T12:00:00Z")), false);
  assert.equal(lifecycle.synchronize(new Date("2026-09-01T12:00:00Z")), false);
  assert.deepEqual(lifecycle.state().query, custom);
  lifecycle.selectPeriod("1", afterMidnight);
  assert.equal(lifecycle.state().period, "1");
  assert.equal(lifecycle.state().query.from, "2026-10-16");
  assert.equal(lifecycle.state().query.to, "2026-10-16");
});

test("late replies are fenced by effective query plus generation, and old numbers are not ready under new dates", () => {
  const lifecycle = new QueryLifecycle("?window=pet", beforeMidnight);
  const yesterday = lifecycle.begin(beforeMidnight);
  const accepted = { revision: yesterday.revision, query: { ...yesterday.query } };
  assert.equal(queryScopeReady(accepted, lifecycle.state()), true);
  // No timer or visibility callback has run: reply settlement itself sees midnight.
  assert.equal(lifecycle.isCurrent(yesterday, afterMidnight), false);
  assert.equal(queryScopeReady(accepted, lifecycle.state()), false);
  const current = lifecycle.begin(afterMidnight);
  assert.equal(lifecycle.isCurrent(current, afterMidnight), true);
  assert.equal(
    queryScopeReady(
      { revision: current.revision, query: { ...current.query } },
      lifecycle.state(),
    ),
    true,
  );
  const latest = lifecycle.begin(afterMidnight);
  assert.equal(lifecycle.isCurrent(current, afterMidnight), false);
  assert.equal(lifecycle.isCurrent(latest, afterMidnight), true);
  lifecycle.update((query) => ({ ...query, model: "other" }), afterMidnight);
  lifecycle.update((query) => ({ ...query, model: undefined }), afterMidnight);
  assert.deepEqual(lifecycle.state().query, { ...latest.query, model: undefined });
  assert.equal(lifecycle.isCurrent(latest, afterMidnight), false);
  assert.equal(queryScopeReady(accepted, lifecycle.state()), false);
});

test("hidden or suspended recovery reuses one view timer, rejects the old reply and immediately queries the resumed day", async () => {
  let now = beforeMidnight;
  let timerId = 0;
  const timers = new Map<number, () => void>();
  const host: IntervalHost = {
    set: (callback) => {
      timers.set(++timerId, callback);
      return timerId;
    },
    clear: (id) => {
      timers.delete(id);
    },
  };
  const lifecycle = new QueryLifecycle("?window=pet", now);
  const requests: ReturnType<QueryLifecycle["begin"]>[] = [];
  const complete: (() => void)[] = [];
  let accepted: Pick<QueryState, "query" | "revision"> | null = null;
  const loop = new ViewRefreshLoop(async () => {
    const ticket = lifecycle.begin(now);
    requests.push(ticket);
    await new Promise<void>((resolve) => complete.push(resolve));
    if (lifecycle.isCurrent(ticket, now))
      accepted = { revision: ticket.revision, query: { ...ticket.query } };
  }, host);
  loop.setVisible(true);
  assert.equal(requests.length, 1);
  assert.equal(timers.size, 1);
  lifecycle.invalidate();
  loop.setVisible(false);
  now = new Date("2026-10-17T01:00:00Z");
  lifecycle.synchronize(now);
  await loop.request();
  assert.equal(requests.length, 1);
  assert.equal(timers.size, 0);
  loop.setVisible(true);
  complete.shift()!();
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(accepted, null);
  assert.equal(requests.length, 2);
  assert.equal(requests[1].query.from, "2026-10-17");
  assert.equal(requests[1].query.to, "2026-10-17");
  assert.equal(timers.size, 1);
  complete.shift()!();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(queryScopeReady(accepted, lifecycle.state()), true);
  loop.dispose();
  assert.equal(timers.size, 0);
});

test("clear and same-ID source rebind revoke accepted facts while preserving every filter for the next read and CSV", () => {
  const lifecycle = new QueryLifecycle("", beforeMidnight);
  lifecycle.selectPeriod("custom", beforeMidnight);
  lifecycle.update({
    from: "2026-10-01", to: "2026-10-15", sourceId: "same-source",
    threadId: "thread", rootTurnId: "root", project: "project", model: "model",
    providerId: "provider", clientKind: "codex", agentKind: "subagent", includeLegacy: true,
  }, beforeMidnight);
  const filters = lifecycle.state();
  for (const action of ["clear_data", "update_source"]) {
    assert.equal(changesQueryDomain(action), true);
    const old = lifecycle.begin(beforeMidnight);
    const accepted = { revision: old.revision, query: { ...old.query } };
    assert.equal(queryScopeReady(accepted, lifecycle.state()), true);
    lifecycle.domainChanged();
    assert.equal(lifecycle.isCurrent(old, beforeMidnight), false);
    assert.equal(queryScopeReady(accepted, lifecycle.state()), false);
    assert.deepEqual(lifecycle.state().query, filters.query);
    assert.equal(lifecycle.state().period, "custom");
    const fresh = lifecycle.begin(beforeMidnight);
    assert.equal(lifecycle.isCurrent(fresh, beforeMidnight), true);
    assert.equal(queryScopeReady({ revision: fresh.revision, query: { ...fresh.query } }, lifecycle.state()), true);
  }
});

test("domain event while an old read is pending rejects both its success and failure authority and coalesces a current read", async () => {
  const lifecycle = new QueryLifecycle("", beforeMidnight);
  let accepted: Pick<QueryState, "query" | "revision"> | null = null;
  let facts = "accepted-before-clear";
  let failure: string | null = null;
  const replies: { resolve: (value: string) => void; reject: (error: Error) => void }[] = [];
  const host: IntervalHost = { set: () => 1, clear: () => undefined };
  const loop = new ViewRefreshLoop(async () => {
    const ticket = lifecycle.begin(beforeMidnight);
    try {
      const result = await new Promise<string>((resolve, reject) => replies.push({ resolve, reject }));
      if (lifecycle.isCurrent(ticket, beforeMidnight)) {
        accepted = { revision: ticket.revision, query: { ...ticket.query } };
        facts = result;
      }
    } catch (error) {
      if (lifecycle.isCurrent(ticket, beforeMidnight)) failure = String(error);
    }
  }, host);
  loop.setVisible(true);
  const oldAccepted = lifecycle.state();
  accepted = oldAccepted;
  assert.equal(isQueryDomainChange({ petDomainChanged: true, reason: "clear_data" }), true);
  lifecycle.domainChanged();
  accepted = null;
  facts = "";
  const completed = loop.request(true);
  replies[0].resolve("old-after-clear");
  await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
  assert.equal(facts, "");
  assert.equal(queryScopeReady(accepted, lifecycle.state()), false);
  assert.equal(replies.length, 2);
  // A second rebind can happen while the first replacement read is in flight.
  lifecycle.domainChanged();
  void loop.request(true);
  replies[1].reject(new Error("stale read failure"));
  await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
  assert.equal(failure, null);
  assert.equal(replies.length, 3);
  replies[2].resolve("current-after-rebind");
  await completed;
  assert.equal(facts, "current-after-rebind");
  assert.equal(queryScopeReady(accepted, lifecycle.state()), true);
  loop.dispose();
});

test("only explicit domain events and the six Native domain mutations revoke a scope", () => {
  for (const action of ["clear_data", "clear_inbox", "set_primary_source", "add_source", "update_source", "relocate_source"])
    assert.equal(changesQueryDomain(action), true);
  for (const action of ["mark_read", "update_settings", "save_budget", "rescan", ""])
    assert.equal(changesQueryDomain(action), false);
  for (const payload of [null, undefined, {}, { petDomainChanged: false }, { petDomainChanged: "true" }, true])
    assert.equal(isQueryDomainChange(payload), false);
});
