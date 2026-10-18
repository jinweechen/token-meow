import assert from "node:assert/strict";
import test from "node:test";
import { demoSnapshot, emptySnapshot } from "./demo";
import {
  accountExpired,
  compact,
  dateTime,
  defaultQuery,
  duration,
  percent,
  petState,
  statusLabel,
  latestTimedTurn,
} from "./format";

test("missing values stay distinct from recorded zeros", () => {
  assert.equal(compact(null), "—");
  assert.equal(compact(0), "0");
  assert.equal(percent(null), "未提供");
  assert.equal(percent(0), "0.0%");
  assert.equal(duration(null), "未提供");
  assert.equal(duration(0), "0 ms");
});
test("parallel active turns take priority over a single completion animation", () => {
  const snapshot = emptySnapshot();
  snapshot.activity.activeTurns = 2;
  assert.equal(petState(snapshot, "completed"), "active");
  snapshot.activity.activeTurns = 0;
  assert.equal(petState(snapshot, "unknown"), "alert");
  assert.equal(petState(snapshot, "failed"), "alert");
  assert.equal(petState(snapshot, "completed"), "complete");
});
test("uncertain tasks and source failures do not become successful completions", () => {
  assert.equal(statusLabel("unrecognized-source-value"), "已结束，结果未知");
  const snapshot = emptySnapshot();
  snapshot.activity.uncertainTurns = 1;
  assert.equal(petState(snapshot), "alert");
  snapshot.activity.uncertainTurns = 0;
  snapshot.settings.doNotDisturb = true;
  assert.equal(petState(snapshot), "sleep");
});
test("display time is explicitly Asia/Shanghai and rejects bad timestamps", () => {
  assert.equal(dateTime("2026-10-14T17:30:00Z", true), "01:30");
  assert.equal(dateTime("bad"), "时间不可用");
});
test("pet small card uses today-only ledger query and panel independently defaults to seven days", () => {
  const now = new Date("2026-10-14T17:30:00Z");
  assert.deepEqual(defaultQuery("?window=pet", now), {
    from: "2026-10-15",
    to: "2026-10-15",
    includeLegacy: false,
  });
  assert.deepEqual(defaultQuery("", now), {
    from: "2026-10-09",
    to: "2026-10-15",
    includeLegacy: false,
  });
});
test("pet browser demo obeys today rather than presenting seven days under a today label", () => {
  const snapshot = demoSnapshot(defaultQuery("?window=pet"));
  assert.equal(snapshot.daily.length, 1);
  assert.equal(snapshot.overview.totalTokens, snapshot.daily[0].totalTokens);
  assert.equal(snapshot.overview.totalTokens, 264_670);
});
test("expired quota retains proportions and is marked stale until a real refresh", () => {
  const account = demoSnapshot().account!;
  const used = account.buckets[0].primary!.usedPercent;
  assert.equal(accountExpired(account), false);
  assert.equal(accountExpired(account, Date.now() + 241_000), true);
  assert.equal(account.buckets[0].primary!.usedPercent, used);
  account.buckets[0].primary!.resetsAt = Math.floor(Date.now() / 1000) - 1;
  assert.equal(accountExpired(account), true);
  assert.equal(account.buckets[0].primary!.usedPercent, used);
});
test("quota freshness requires a valid collection time and survives clock rollback safely", () => {
  const now = Date.parse("2026-10-16T06:00:00Z");
  const account = demoSnapshot().account!;
  account.buckets.forEach((bucket) => {
    if (bucket.primary) bucket.primary.resetsAt = null;
    if (bucket.secondary) bucket.secondary.resetsAt = null;
  });
  const used = account.buckets[0].primary!.usedPercent;
  for (const collectedAt of [null, "invalid", new Date(now + 1).toISOString()]) {
    account.collectedAt = collectedAt;
    assert.equal(accountExpired(account, now), true);
    assert.equal(account.buckets[0].primary!.usedPercent, used);
  }
  account.collectedAt = new Date(now - 239_999).toISOString();
  assert.equal(accountExpired(account, now), false);
  account.collectedAt = new Date(now - 240_000).toISOString();
  assert.equal(accountExpired(account, now), true);
  account.collectedAt = new Date(now).toISOString();
  assert.equal(accountExpired(account, now), false);
  assert.equal(accountExpired(account, Number.NaN), true);
});
test("missing account data remains unavailable rather than an expired quota", () => {
  assert.equal(accountExpired(undefined), false);
  assert.equal(
    accountExpired({ status: "unavailable", collectedAt: null, buckets: [] }),
    false,
  );
  assert.equal(
    accountExpired({ status: "loading", collectedAt: null, buckets: [] }),
    false,
  );
});
test("latest turn missing TTFT stays missing instead of borrowing an earlier measurement", () => {
  const snapshot = demoSnapshot();
  const previous = {
    ...snapshot.turns[0],
    completedAt: "2026-10-15T08:00:00Z",
    timeToFirstTokenMs: 1200,
  };
  const latest = {
    ...snapshot.turns[1],
    completedAt: "2026-10-15T17:00:00+08:00",
    timeToFirstTokenMs: null,
  };
  assert.equal(latestTimedTurn([previous, latest])?.id, latest.id);
  assert.equal(latestTimedTurn([previous, latest])?.timeToFirstTokenMs, null);
});
