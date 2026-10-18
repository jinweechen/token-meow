import assert from "node:assert/strict";
import test from "node:test";
import { officialUsageDays } from "./accountUsage";
import type { AccountSnapshot } from "./contracts";

test("official daily buckets preserve source dates, gaps, duplicate observations and unknown values", () => {
  const account: AccountSnapshot = {
    status: "connected",
    collectedAt: null,
    buckets: [],
    usage: {
      dailyUsageBuckets: [
        { startDate: "2026-10-15", tokens: null },
        { startDate: "2026-10-12", tokens: 0 },
        { startDate: "2026-10-15", tokens: 123 },
        { startDate: "2026-02-30", tokens: 999 },
        { startDate: "2026-10-13T00:00:00Z", tokens: 999 },
        { startDate: "2026-10-14", tokens: -1 },
      ],
    },
  };
  assert.deepEqual(officialUsageDays(account), [
    { startDate: "2026-10-12", tokens: 0 },
    { startDate: "2026-10-14", tokens: null },
    { startDate: "2026-10-15", tokens: null },
    { startDate: "2026-10-15", tokens: 123 },
  ]);
  assert.equal(account.usage!.dailyUsageBuckets!.length, 6);
});
test("missing official trend remains unavailable while an explicit empty list remains empty", () => {
  assert.equal(officialUsageDays(undefined), null);
  const account: AccountSnapshot = {
    status: "unavailable",
    collectedAt: null,
    buckets: [],
    usage: null,
  };
  assert.equal(officialUsageDays(account), null);
  account.usage = { dailyUsageBuckets: [] };
  assert.deepEqual(officialUsageDays(account), []);
});
