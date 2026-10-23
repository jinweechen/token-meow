import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { SubscriptionView } from "../features/subscription/SubscriptionView";
import type { QuotaCalibration, SubscriptionOverview } from "./subscriptionContracts";

function calibration(): QuotaCalibration {
  return { id: "shared:primary", windowId: "shared:primary", model: null, label: "当前模型组合", quality: "conditional",
    capacityLower: null, capacityUpper: null, empiricalCapacity: 1_000_000, remainingLower: null, remainingUpper: null,
    sampleCount: 3, cycleCount: 1, coverage: "verified_complete", structure: "相同响应模型、effort、速度与缓存结构", reasons: ["precision_unknown"] };
}
function render(value: QuotaCalibration) {
  const overview: SubscriptionOverview = { schemaVersion: 1, revision: 1, asOf: "2026-10-22T02:00:00Z", source: { id: "source-a", label: "当前来源" }, membership: null,
    capabilities: { officialQuota: true, manualPlans: true, capacityAssumptions: true, drafts: true, nativeNotifications: false },
    windows: [], modelUsage: [], plans: [], calibrations: [value], assumptions: [], notifications: { channel: "panel", qualified: false, reasons: [] }, reasons: [] };
  return renderToStaticMarkup(createElement(SubscriptionView, { overview, onEditPlan() {}, onToggleReminders() {}, onEndPlan() {} }));
}

test("unknown precision capacity shows one explicitly unbounded empirical point", () => {
  const html = render(calibration());
  for (const label of ["精度未知的经验情景", "约 1,000,000 Token", "完整窗口经验点", "误差区间、剩余 Token 和耗尽时间仍不可估", "不参与计划可持续性验证或提醒", "条件参考"]) assert.ok(html.includes(label), label);
  assert.equal((html.match(/<dd>暂不可估<\/dd>/g) ?? []).length, 2, "the point must not populate capacity or remaining intervals");
  assert.ok(!html.includes("跨周期验证通过"));
});

test("known quantization intervals take precedence without an extra point", () => {
  const html = render({ ...calibration(), capacityLower: 900_000, capacityUpper: 1_100_000, empiricalCapacity: 3_333_333, reasons: [] });
  assert.ok(html.includes("900,000～1,100,000 Token"));
  assert.ok(!html.includes("精度未知的经验情景"));
  assert.ok(!html.includes("3,333,333"));
});

test("empirical points reject invalid quality, missing precision reason and unsafe values", () => {
  for (const quality of ["learning", "validated", "invalidated"] as const) assert.ok(!render({ ...calibration(), quality }).includes("精度未知的经验情景"), quality);
  assert.ok(!render({ ...calibration(), reasons: [] }).includes("精度未知的经验情景"));
  for (const empiricalCapacity of [null, undefined, 0, -1, NaN, Infinity, 9_007_199_254_740_992]) {
    assert.ok(!render({ ...calibration(), empiricalCapacity }).includes("精度未知的经验情景"), String(empiricalCapacity));
  }
});

test("partial capacity or remaining intervals cannot be relabelled as an empirical point", () => {
  for (const field of ["capacityLower", "capacityUpper", "remainingLower", "remainingUpper"] as const) {
    assert.ok(!render({ ...calibration(), [field]: 1 }).includes("精度未知的经验情景"), field);
  }
});

test("archived empirical points retain their historical disclosure and cannot represent current balance", () => {
  const html = render({ ...calibration(), archived: true, recordedAt: "2026-10-20T02:00:00Z" });
  for (const label of ["历史校准记录", "不代表当前余额", "精度未知的经验情景", "约 1,000,000 Token"]) assert.ok(html.includes(label), label);
  assert.ok(html.includes('<details class="subscription-scenario">'));
  assert.ok(!html.includes("<details open"));
  assert.equal((html.match(/<dd>暂不可估<\/dd>/g) ?? []).length, 2);
});

test("a single-model empirical scenario stays explicit and mutually exclusive", () => {
  const html = render({ ...calibration(), model: "actual-model" });
  assert.ok(html.includes("假设全部用于 actual-model（与其他模型情景互斥）"));
  assert.ok(html.includes("精度未知的经验情景"));
  assert.ok(!html.includes("<details open"));
});
