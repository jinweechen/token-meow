import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { SubscriptionView, UsagePlanDraftBasis } from "../features/subscription/SubscriptionView";
import type { LocalPlanForecast, SubscriptionOverview, UsagePlan } from "./subscriptionContracts";
import { subscriptionBytes, subscriptionLocalForecastRelation, subscriptionLocalForecastValue, subscriptionPlanForecast, subscriptionReason } from "./subscriptionPresentation";

function forecast(kind: LocalPlanForecast["kind"]): LocalPlanForecast {
  return {
    kind, state: "conditional", scope: "local-plan", planId: "plan-a", sourceId: "source-a", modelDimension: "response", model: null,
    planConfigurationRevision: 2, asOfRevision: 777, revisionKind: "subscription-domain-revision", inputLedgerFingerprint: "ledger-fingerprint-a", asOf: "2026-10-22T02:00:00Z", periodEndsAt: "2026-10-22T04:00:00Z", quality: "conditional-local-experiment",
    notifyEligible: false, uncertaintyBounds: null,
    empiricalScenarioRange: kind === "local_plan_eta" ? { earliestAt: "2026-10-22T02:10:00Z", latestAt: "2026-10-22T02:30:00Z" } : { minimum: 900, maximum: 1300, unit: "local-plan-tokens" },
    relationToPeriodEnd: kind === "local_plan_eta" ? "before" : null, willReachBeforePeriodEnd: kind === "local_plan_eta" ? true : null, reasons: [],
  };
}
function plan(): UsagePlan {
  return { id: "plan-a", revision: 2, title: "本机目标", type: "tokens", sourceId: "source-a", model: null, modelDimension: "response", target: 1000, effectiveFrom: "2026-10-22T01:00:00Z", endPolicy: "time", endsAt: "2026-10-22T04:00:00Z", windowId: null, lifecycle: "active", evaluationState: "ready", remindersEnabled: false, used: 300, remaining: 700, progressPercent: 30, reasons: [], forecasts: { localPlanEta: forecast("local_plan_eta"), periodEndUsage: forecast("period_end_usage") } };
}
function overview(value: UsagePlan): SubscriptionOverview {
  return { schemaVersion: 1, revision: 1, asOf: "2026-10-22T02:00:00Z", source: { id: "source-a", label: "本机来源" }, membership: null, capabilities: { officialQuota: false, manualPlans: true, capacityAssumptions: false, drafts: true, nativeNotifications: false }, windows: [], modelUsage: [], plans: [value], calibrations: [], assumptions: [], notifications: { channel: "panel", qualified: false, reasons: [] }, reasons: [] };
}
function render(value: SubscriptionOverview) {
  return renderToStaticMarkup(createElement(SubscriptionView, { overview: value, onEditPlan() {}, onToggleReminders() {}, onEndPlan() {} }));
}

test("local forecasts require the current plan, source, model dimension and revision", () => {
  const value = plan();
  assert.equal(subscriptionPlanForecast(value, "local_plan_eta"), value.forecasts!.localPlanEta);
  assert.notEqual(value.forecasts!.localPlanEta.asOfRevision, value.revision, "input ledger revisions are independent of plan configuration revisions");
  assert.equal(subscriptionLocalForecastValue(subscriptionPlanForecast(value, "local_plan_eta")), "10/22 10:10～10/22 10:30");
  for (const changed of [{ id: "plan-b" }, { sourceId: "source-b" }, { model: "other" }, { modelDimension: "request" as const }, { revision: 3 }, { type: "percent" as const }, { lifecycle: "ended" as const }, { evaluationState: "unknown" as const }]) {
    assert.equal(subscriptionPlanForecast({ ...value, ...changed }, "local_plan_eta"), null, JSON.stringify(changed));
  }
  assert.equal(subscriptionPlanForecast({ ...value, forecasts: undefined }, "period_end_usage"), null);
});

test("local empirical scenarios preserve null, zero and invalid values", () => {
  const eta = forecast("local_plan_eta");
  const end = forecast("period_end_usage");
  assert.equal(subscriptionLocalForecastValue(eta), "10/22 10:10～10/22 10:30");
  assert.equal(subscriptionLocalForecastValue(end), "900～1,300 Token");
  assert.equal(subscriptionLocalForecastValue({ ...end, empiricalScenarioRange: { minimum: 0, maximum: 0, unit: "local-plan-tokens" } }), "0～0 Token");
  for (const state of ["unavailable", "unknown"] as const) {
    assert.equal(subscriptionLocalForecastValue({ ...eta, state }), "暂不可估");
    assert.equal(subscriptionLocalForecastValue({ ...end, state }), "暂不可估");
  }
  for (const value of [null, { ...eta, asOf: null }, { ...eta, asOf: "invalid" }, { ...eta, empiricalScenarioRange: null }, { ...eta, empiricalScenarioRange: { earliestAt: "2026-10-22T03:00:00Z", latestAt: "2026-10-22T02:00:00Z" } }, { ...eta, empiricalScenarioRange: { earliestAt: "invalid", latestAt: "2026-10-22T02:00:00Z" } }]) assert.equal(subscriptionLocalForecastValue(value), "暂不可估");
  for (const minimum of [NaN, Infinity, -1, 1.1, 9007199254740992]) assert.equal(subscriptionLocalForecastValue({ ...end, empiricalScenarioRange: { minimum, maximum: 9007199254740992, unit: "local-plan-tokens" } }), "暂不可估");
  assert.equal(subscriptionLocalForecastValue({ ...end, empiricalScenarioRange: { minimum: 1, maximum: 2, unit: "official-tokens" } }), "暂不可估");
});

test("reached local targets display no invented exhaustion time or period judgment", () => {
  const value = { ...forecast("local_plan_eta"), state: "reached" as const, empiricalScenarioRange: null, relationToPeriodEnd: null, willReachBeforePeriodEnd: null };
  assert.equal(subscriptionLocalForecastValue(value), "目标已达到");
  assert.equal(subscriptionLocalForecastRelation(value), null);
  assert.equal(subscriptionLocalForecastValue({ ...value, kind: "period_end_usage" }), "暂不可估");
});

test("period-end judgments follow both Core fields without deriving a guarantee", () => {
  const value = forecast("local_plan_eta");
  assert.equal(subscriptionLocalForecastRelation(value), "经验情景均在计划截止前达到目标");
  assert.equal(subscriptionLocalForecastRelation({ ...value, relationToPeriodEnd: "after", willReachBeforePeriodEnd: false }), "经验情景均在计划截止后达到目标");
  assert.equal(subscriptionLocalForecastRelation({ ...value, relationToPeriodEnd: "overlapping", willReachBeforePeriodEnd: null }), "经验情景跨过截止时间，是否提前达到目标暂不确定");
  assert.equal(subscriptionLocalForecastRelation({ ...value, relationToPeriodEnd: "before", willReachBeforePeriodEnd: false }), "当前无法判断是否在截止前达到目标");
  assert.equal(subscriptionLocalForecastRelation({ ...value, empiricalScenarioRange: null }), null);
});

test("Token plan cards show two local scenarios while paused reminders keep counting", () => {
  const html = render(overview(plan()));
  for (const label of ["本机目标耗尽情景（上海时区）", "计划截止时本机累计用量情景", "900～1,300 Token", "经验情景均在计划截止前达到目标", "不是保证或置信区间", "采样基准", "提醒已暂停，仍持续计量"]) assert.ok(html.includes(label), label);
  assert.equal((html.match(/<button\b/g) ?? []).length, 3, "forecasts must add no reminder controls");
  assert.ok(html.includes('data-forecast-kind="local_plan_eta"'));
  assert.ok(html.includes('data-forecast-kind="period_end_usage"'));
});

test("unavailable or out-of-scope forecasts cannot leak numeric scenario payloads", () => {
  for (const mismatch of [false, true]) {
    const value = plan();
    for (const forecast of Object.values(value.forecasts!)) {
      forecast.state = mismatch ? "conditional" : "unavailable";
      forecast.sourceId = mismatch ? "old-source" : "source-a";
      forecast.empiricalScenarioRange = { minimum: 888888, maximum: 999999, unit: "local-plan-tokens", earliestAt: "2099-01-01T00:00:00Z", latestAt: "2099-01-02T00:00:00Z" };
      forecast.reasons = ["local_forecast_samples_insufficient"];
    }
    const html = render(overview(value));
    assert.equal((html.match(/<dd>暂不可估<\/dd>/g) ?? []).length, 2);
    for (const value of ["888,888", "999,999", "01/01", "经验情景均在"]) assert.ok(!html.includes(value), value);
  }
});

test("historical-demand drafts label P50 and P90 as demand rather than usable quota", () => {
  const html = renderToStaticMarkup(createElement(UsagePlanDraftBasis, { draft: { kind: "historical_demand", plan: null, basis: ["5 个完整历史时段"], constraintsVerified: false, reasons: ["quota_sustainability_unverified"] } }));
  for (const label of ["历史需求草案", "历史 P50 用于建议本机目标", "P90 用于高需求参考", "厂商额度可持续性仍待验证", "额度约束未验证", "可直接新建手动计划"]) assert.ok(html.includes(label), label);
});

test("automatic learning availability and local forecast reasons use actionable Chinese", () => {
  const value = overview(plan());
  value.learningStatus = { status: "unavailable", reasonCodes: ["adapter_quota_proof_missing"], assembledPairs: 0 };
  const html = render(value);
  assert.ok(html.includes("当前自动校准暂不可用"));
  assert.ok(html.includes("暂不能自动换算会员 Token 容量"));
  const codes = ["local_forecast_scope_unknown", "local_forecast_view_mismatch", "local_forecast_coverage_unknown", "local_forecast_period_ended", "local_forecast_usage_unknown", "local_forecast_target_unknown", "local_forecast_measurement_unknown", "local_forecast_measurement_stale", "local_forecast_clock_discontinuous", "local_forecast_scan_unreliable", "local_forecast_runtime_changed", "local_forecast_scope_changed", "local_forecast_revision_regressed", "local_forecast_usage_reduced", "local_forecast_samples_insufficient", "local_forecast_measurement_unobserved", "local_forecast_sample_expired", "local_forecast_recent_change_missing", "local_forecast_numeric_overflow"];
  for (const code of codes) assert.match(subscriptionReason(code), /[\u3400-\u9fff]/u, code);
});

test("evidence availability distinguishes collection time, retained payload and capacity trimming", () => {
  const value = overview(plan());
  value.evidenceAvailability = {
    schemaVersion: 1, sourceId: "source-a", earliestAvailableAt: "2026-10-21T00:00:00Z", earliestRecordedFactAt: null, timeBasis: "collected-at-not-provider-fact-time",
    receiptCount: 25, fullEnvelopeCount: 10, referenceReceiptCount: 15, unresolvedReferenceCount: 2, payloadBytes: 1048576, retentionDays: 30,
    limits: { perSourceReceipts: 100000, perSourcePayloadBytes: 33554432, totalPayloadBytes: 134217728, maxDays: 90 },
    capacityTruncation: { scope: "all-subscription-evidence", lastAppliedAt: "2026-10-22T01:00:00Z", deletedByAge: 3, deletedBySourceCount: 4, deletedBySourceBytes: 5, deletedByGlobalBytes: 6, deletedTotal: 18, baseDependenciesMissing: 2 }, integrityCheckedOnResolve: true, payloadIsSqliteDiskSize: false,
  };
  const html = render(value);
  for (const label of ["证据保留范围与容量", "最早可用证据的本机采集时间", "最早明确记载的厂商事实时间", "25 条", "1 MiB", "30 天", "32 MiB", "128 MiB", "最近一次全局订阅证据清理", "按保留期移除 3 条", "按容量移除 15 条", "2 条观测缺少原始依据", "不等于数据库文件的磁盘占用"]) assert.ok(html.includes(label), label);
  assert.ok(!html.includes("<details open"), "retention details must not crowd out plan and quota values");
  value.evidenceAvailability.sourceId = "old-source";
  assert.ok(!render(value).includes("证据保留范围与容量"), "another source's evidence summary must not be shown");
});

test("evidence content bytes preserve zero and refuse unsafe or missing sizes", () => {
  assert.equal(subscriptionBytes(0), "0 B");
  assert.equal(subscriptionBytes(1024), "1 KiB");
  assert.equal(subscriptionBytes(33554432), "32 MiB");
  for (const value of [null, undefined, -1, NaN, Infinity, 1.1, 9007199254740992]) assert.equal(subscriptionBytes(value), "未知");
});
