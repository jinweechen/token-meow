import assert from "node:assert/strict";
import test from "node:test";
import {
  shanghaiDateTimeInput,
  shanghaiEffectiveFrom,
  usageDay,
  usageNumber,
  usageTime,
  usageUsd,
  validPriceDecimal,
} from "./usageDashboard";
import { demoSnapshot, emptySnapshot } from "./demo";
import { demoUsageDashboard } from "./usageDashboardDemo";

test("dashboard preserves missing counters and unpriced estimates separately from observed zero", () => {
  assert.equal(usageNumber(null), "未提供");
  assert.equal(usageNumber(0), "0");
  assert.equal(usageNumber(2_160_000_000, true), "2.16B");
  assert.equal(usageUsd(null), "未定价");
  assert.equal(usageUsd("0"), "$0.00");
  assert.equal(usageUsd("0.000000001"), "< $0.00000001");
  assert.equal(usageUsd("0.00000312"), "$0.00000312");
  assert.equal(usageUsd("bad"), "未提供");
});

test("price editor uses Shanghai dates independently of host timezone and rejects impossible wall times", () => {
  assert.equal(
    shanghaiDateTimeInput(new Date("2026-10-16T17:30:00Z")),
    "2026-10-17T01:30",
  );
  assert.equal(
    shanghaiEffectiveFrom("2026-10-17T01:30"),
    "2026-10-17T01:30:00+08:00",
  );
  assert.equal(shanghaiEffectiveFrom("2026-02-30T01:30"), null);
  assert.equal(shanghaiEffectiveFrom("2026-10-17T25:30"), null);
  assert.equal(shanghaiEffectiveFrom("2026-10-17"), null);
  assert.equal(usageTime("2026-10-16T17:30:00Z"), "01:30:00");
  assert.equal(usageDay("2026-10-16T17:30:00Z"), "10/17");
  assert.equal(usageDay("invalid"), "时间不可用");
});

test("dashboard preview derives totals, groups, price coverage and trend from the same response rows", () => {
  const snapshot = demoSnapshot();
  const first = demoUsageDashboard(
    snapshot,
    {},
    { page: 0, pageSize: 100, quality: "all" },
  );
  const second = demoUsageDashboard(
    snapshot,
    {},
    { page: 1, pageSize: 100, quality: "all" },
  );
  const rows = [...first.rows, ...second.rows];
  assert.equal(rows.length, first.totalRows);
  assert.equal(new Set(rows.map((row) => row.id)).size, first.totalRows);
  assert.equal(
    rows.reduce((total, row) => total + (row.totalTokens ?? 0), 0),
    first.summary.totalTokens,
  );
  assert.equal(
    first.trend.reduce((total, point) => total + point.totalTokens, 0),
    first.summary.totalTokens,
  );
  assert.equal(
    first.models.reduce((total, model) => total + model.totalTokens, 0),
    first.summary.totalTokens,
  );
  assert.equal(first.providers[0].name, "未知");
  assert.equal(
    first.summary.pricedResponses + first.summary.unpricedResponses,
    first.summary.trustedMeteredResponseCount,
  );
  const cost = rows.reduce(
    (total, row) => total + Math.round(Number(row.estimatedCostUsd ?? 0) * 1e9),
    0,
  );
  assert.equal(cost, Math.round(Number(first.summary.estimatedCostUsd) * 1e9));
  const unpriced = demoUsageDashboard(
    snapshot,
    {},
    { page: 0, pageSize: 100, quality: "unpriced" },
  );
  assert.deepEqual(unpriced.summary, first.summary);
  assert.deepEqual(unpriced.trend, first.trend);
  assert.equal(unpriced.totalRows, first.summary.unpricedResponses);
  assert.ok(unpriced.rows.every((row) => row.estimatedCostUsd == null));
});

test("dashboard preview obeys date, model, identity filters and privacy while pagination starts at zero", () => {
  const snapshot = demoSnapshot();
  const day = snapshot.daily.at(-1)!.date;
  const oneDay = demoUsageDashboard(
    snapshot,
    {
      from: day,
      to: day,
      model: "gpt-6-astra",
      sourceId: "demo-source",
      clientKind: "codex",
      providerId: "unknown",
      agentKind: "primary",
    },
    { page: 0, pageSize: 20, quality: "all" },
  );
  assert.equal(oneDay.granularity, "hour");
  assert.equal(oneDay.trend.length, 24);
  assert.equal(oneDay.request.page, 0);
  assert.ok(oneDay.rows.length > 0);
  assert.ok(
    oneDay.rows.every(
      (row) =>
        row.observedModel === "gpt-6-astra" && row.eventTime.startsWith(day),
    ),
  );
  assert.equal(oneDay.summary.estimatedCostUsd, null);
  assert.equal(
    demoUsageDashboard(
      snapshot,
      { sourceId: "not-demo" },
      { page: 0, pageSize: 20, quality: "all" },
    ).totalRows,
    0,
  );
  assert.equal(
    demoUsageDashboard(
      snapshot,
      { clientKind: "other" },
      { page: 0, pageSize: 20, quality: "all" },
    ).totalRows,
    0,
  );
  snapshot.settings.privacyMode = true;
  const privateView = demoUsageDashboard(
    snapshot,
    {},
    { page: 0, pageSize: 20, quality: "all" },
  );
  assert.ok(
    privateView.rows.every(
      (row) => row.threadId == null && row.responseId == null,
    ),
  );
});

test("price validation accepts explicit zero and decimals while rejecting unsafe or ambiguous forms", () => {
  for (const price of ["0", "1.25", "0.000001", "1000000", " 2.5 "])
    assert.equal(validPriceDecimal(price), true, price);
  for (const price of [
    "",
    "-1",
    "1e6",
    "Infinity",
    "NaN",
    ".5",
    "01",
    "1,000",
    "1000001",
    "0.0000001",
  ])
    assert.equal(validPriceDecimal(price), false, price);
});

test("dashboard preview keeps empty snapshots empty and preserves an out-of-range requested page", () => {
  const request = { page: 0, pageSize: 20, quality: "all" as const };
  const empty = demoUsageDashboard(emptySnapshot(), {}, request);
  assert.equal(empty.totalRows, 0);
  assert.equal(empty.rows.length, 0);
  assert.equal(empty.summary.totalTokens, null);
  assert.equal(empty.summary.estimatedCostUsd, null);
  assert.equal(empty.pricingVersions.length, 0);
  assert.equal(empty.models.length, 0);
  const beyond = demoUsageDashboard(
    demoSnapshot(),
    {},
    { ...request, page: 500 },
  );
  assert.equal(beyond.request.page, 500);
  assert.equal(beyond.rows.length, 0);
  assert.ok(beyond.totalRows > 0);
});
