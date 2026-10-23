import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { SubscriptionView } from "../features/subscription/SubscriptionView";
import { createSubscriptionClient, type SubscriptionInvoke } from "./subscriptionClient";
import type { SubscriptionOverview, UsagePlan } from "./subscriptionContracts";
import { buildAssumptionInput, buildPlanInput, newPlanForm, subscriptionDateInput, subscriptionDateValue } from "./subscriptionForms";
import { subscriptionCoverage, subscriptionModelName, subscriptionNumber, subscriptionProvenance, subscriptionRange, subscriptionReason, subscriptionTime } from "./subscriptionPresentation";

const now = Date.parse("2026-10-22T02:00:00.000Z");
function overview(): SubscriptionOverview {
  return {
    schemaVersion: 1, revision: 7, asOf: new Date(now).toISOString(), source: { id: "source-a", label: "当前 Codex" },
    membership: null,
    capabilities: { officialQuota: false, manualPlans: true, capacityAssumptions: true, drafts: true, nativeNotifications: false },
    windows: [{ id: "shared:primary", bucketId: "shared", bucketName: "共同权益", label: "实际短窗口", periodInstanceId: "period-1", usedPercent: 60, remainingPercent: 40, resetsAt: "2026-10-22T04:00:00.000Z", resetState: "current", freshness: "fresh", canBindPlan: true, reasons: [] },
      { id: "shared:secondary", bucketId: "shared", bucketName: "共同权益", label: "实际长窗口", periodInstanceId: null, usedPercent: null, remainingPercent: null, resetsAt: null, resetState: "waiting", freshness: "stale", canBindPlan: false, reasons: ["waiting_reset"] },
      { id: "extra:primary", bucketId: "extra", bucketName: "模型附加池", label: "模型附加窗口", periodInstanceId: null, usedPercent: 0, remainingPercent: 100, resetsAt: null, resetState: "unknown", freshness: "fresh", canBindPlan: false, reasons: [] }],
    modelUsage: [{ model: "actual-model", modelDimension: "response", tokens: null, from: "2026-10-22T00:00:00Z", to: "2026-10-22T02:00:00Z", coverage: "部分覆盖", reasons: ["identity_unknown"] }, { model: "requested-model", modelDimension: "request", tokens: 0, from: null, to: null, coverage: "来源记录", reasons: [] }],
    plans: [], calibrations: [], assumptions: [], notifications: { channel: "panel", qualified: true, reasons: ["native_not_qualified"] }, reasons: ["identity_consistency_unproven"],
  };
}
function existingPlan(): UsagePlan {
  return { id: "plan-a", revision: 4, title: "进行中的计划", type: "tokens", sourceId: "source-a", model: "actual-model", modelDimension: "response", target: 100000, effectiveFrom: "2026-10-22T01:01:02.333Z", endPolicy: "time", endsAt: "2026-10-22T03:11:22.999Z", windowId: null, lifecycle: "active", evaluationState: "ready", remindersEnabled: true, used: 25000, remaining: 75000, progressPercent: 25, reasons: [] };
}

test("subscription values preserve unknown versus zero and outward interval bounds", () => {
  for (const value of [null, undefined, NaN, Infinity, -Infinity]) assert.equal(subscriptionNumber(value), "未知");
  assert.equal(subscriptionNumber(0, "%"), "0%");
  assert.equal(subscriptionRange(4545.45, 16666.67), "4,545～16,667 Token");
  assert.equal(subscriptionRange(null, 2), "暂不可估");
  assert.equal(subscriptionRange(3, 2), "暂不可估");
  assert.equal(subscriptionTime(null), "未知");
  assert.equal(subscriptionTime("not a date"), "未知");
  assert.equal(subscriptionReason("identity_unknown"), "账号身份未知");
  assert.equal(subscriptionReason("new_core_reason"), "new_core_reason");
  assert.equal(subscriptionCoverage("recorded-local"), "本机已记录，非全账号覆盖");
  assert.equal(subscriptionProvenance("official-observation"), "厂商观测");
  assert.equal(subscriptionModelName("unknown"), "未知模型");
  for (const reason of ["identity_unproven", "stale_observation", "recovery_unknown", "collection_incomplete", "evidence_removed", "coverage_unknown", "usage_conflict_excluded", "window_confirmation_pending", "source_unavailable", "model_unknown", "model_coverage_incomplete", "native_qualification_pending", "comparable_covered_intervals_insufficient", "related_pool_constraints_unverified"]) assert.notEqual(subscriptionReason(reason), reason);
});

test("from-now source Token plans remain available without official identity or capacity", () => {
  const value = overview();
  value.windows = [];
  const form = { ...newPlanForm(now), target: "500000", model: "", title: "两小时单次计划" };
  const input = buildPlanInput(form, value, null, now);
  assert.deepEqual(input, { expectedRevision: null, title: "两小时单次计划", type: "tokens", sourceId: "source-a", model: null, modelDimension: "response", target: 500000, endPolicy: "time", endsAt: "2026-10-22T04:00:00.000Z", windowId: null, remindersEnabled: true });
  assert.equal("effectiveFrom" in input, false);
  assert.equal("capacityTokens" in input, false);
});

test("goal revision preserves immutable source, dimension and precise original end", () => {
  const plan = existingPlan();
  const forgedForm = { ...newPlanForm(now), type: "percent" as const, model: "requested-model", modelDimension: "request" as const, target: "50000", endPolicy: "window" as const, windowId: "shared:secondary", title: "较低目标" };
  const input = buildPlanInput(forgedForm, overview(), plan, now);
  assert.equal(input.expectedRevision, 4);
  assert.equal(input.type, "tokens");
  assert.equal(input.model, "actual-model");
  assert.equal(input.modelDimension, "response");
  assert.equal(input.endPolicy, "time");
  assert.equal(input.endsAt, "2026-10-22T03:11:22.999Z");
  assert.equal(input.windowId, null);
  assert.equal(input.target, 50000);
  assert.equal("effectiveFrom" in input, false);
  assert.throws(() => buildPlanInput(forgedForm, overview(), { ...plan, lifecycle: "ended" }, now), /已结束/);
  assert.throws(() => buildPlanInput(forgedForm, overview(), { ...plan, sourceId: "old-source" }, now), /不属于当前来源/);
});

test("official goals retain window occupancy target and refuse unconfirmed windows", () => {
  const form = { ...newPlanForm(now), type: "percent" as const, target: "80", windowId: "shared:primary" };
  const input = buildPlanInput(form, overview(), null, now);
  assert.equal(input.target, 80);
  assert.equal(input.endPolicy, "window");
  assert.equal(input.endsAt, null);
  assert.equal(input.model, null);
  assert.throws(() => buildPlanInput({ ...form, windowId: "shared:secondary" }, overview(), null, now), /可绑定/);
  assert.throws(() => buildPlanInput({ ...form, target: "101" }, overview(), null, now), /不超过 100/);
});

test("plan inputs validate safe Token integers, date boundaries and actual model dimension", () => {
  const form = { ...newPlanForm(now), target: "1000" };
  for (const target of ["", "0", "-1", "Infinity", "NaN", "1.2", "9007199254740992"]) assert.throws(() => buildPlanInput({ ...form, target }, overview(), null, now));
  assert.throws(() => buildPlanInput({ ...form, model: "actual-model", modelDimension: "request" }, overview(), null, now), /实际记录/);
  const unknown = overview(); unknown.modelUsage.push({ ...unknown.modelUsage[0], model: "unknown" });
  assert.throws(() => buildPlanInput({ ...form, model: "unknown" }, unknown, null, now), /实际记录/);
  assert.throws(() => buildPlanInput({ ...form, endsAt: "2026-10-22T09:59" }, overview(), null, now), /未来/);
  assert.equal(subscriptionDateValue("2026-02-30T12:00"), null);
  assert.equal(subscriptionDateValue("2026-10-22T12:00"), "2026-10-22T04:00:00.000Z");
  assert.equal(subscriptionDateInput("2026-10-22T04:00:00Z"), "2026-10-22T12:00");
});

test("manual capacity evidence is independent and requires stated premises and expiry", () => {
  const form = { windowId: "shared:primary", model: "", capacityTokens: "2000000", validUntil: "2026-10-23T10:00", note: "仅作为当前会员及模型组合下的手动假设" };
  const input = buildAssumptionInput(form, overview(), now);
  assert.equal(input.capacityTokens, 2000000);
  assert.equal(input.expectedRevision, null);
  assert.equal("target" in input, false);
  assert.equal("quality" in input, false);
  const edited = buildAssumptionInput({ ...form, capacityTokens: "2500000" }, overview(), now, { ...input, id: "assumption-a", revision: 2 });
  assert.equal(edited.id, "assumption-a");
  assert.equal(edited.expectedRevision, 2);
  assert.equal(edited.capacityTokens, 2500000);
  assert.equal("target" in edited, false);
  assert.throws(() => buildAssumptionInput({ ...form, note: " " }, overview(), now), /说明容量依据/);
  assert.throws(() => buildAssumptionInput({ ...form, validUntil: "2026-10-21T10:00" }, overview(), now), /未来/);
});

test("subscription client uses finite independent commands and preserves domain rejection", async () => {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const call: SubscriptionInvoke = async <T>(command: string, args?: Record<string, unknown>) => { calls.push({ command, args }); return overview() as T; };
  const client = createSubscriptionClient(call);
  const plan = buildPlanInput({ ...newPlanForm(now), target: "1000" }, overview(), null, now);
  const assumption = buildAssumptionInput({ windowId: "shared:primary", model: "", capacityTokens: "2000000", validUntil: "2026-10-23T10:00", note: "明确的手动假设" }, overview(), now);
  const draft = { intent: "remaining" as const, sourceId: "source-a", windowId: "shared:primary", model: null, modelDimension: "response" as const, reservePercent: 20 };
  await client.readOverview(); await client.savePlan(plan); await client.setPlanStatus({ id: "plan-a", expectedRevision: 4, remindersEnabled: false }); await client.saveAssumption(assumption); await client.generateDraft(draft); await client.refreshOfficial();
  assert.deepEqual(calls, [{ command: "read_subscription_overview", args: undefined }, { command: "save_usage_plan", args: { payload: plan } }, { command: "set_usage_plan_status", args: { payload: { id: "plan-a", expectedRevision: 4, remindersEnabled: false } } }, { command: "save_quota_assumption", args: { payload: assumption } }, { command: "generate_usage_plan_draft", args: { payload: draft } }, { command: "refresh_account", args: undefined }]);
  assert.equal(calls.filter((entry) => entry.command === "save_usage_plan").length, 1, "draft generation must not save or invoke another model");
  const rejected = createSubscriptionClient(async () => { throw Error("配置修订冲突"); });
  await assert.rejects(() => rejected.savePlan(plan), /配置修订冲突/);
});

test("all four scoped subscription operations preserve the caller's captured view scope", async () => {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const client = createSubscriptionClient(async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args }); return overview() as T;
  });
  const plan = buildPlanInput({ ...newPlanForm(now), target: "1000" }, overview(), null, now);
  const status = { id: "plan-a", expectedRevision: 4, remindersEnabled: false };
  const assumption = buildAssumptionInput({ windowId: "shared:primary", model: "", capacityTokens: "2000000", validUntil: "2026-10-23T10:00", note: "明确的手动假设" }, overview(), now);
  const draft = { intent: "habit" as const, sourceId: "source-a", windowId: null, model: null, modelDimension: "response" as const, reservePercent: 20 };
  await client.savePlan(plan, "plan-form-scope");
  await client.setPlanStatus(status, "status-view-scope");
  await client.saveAssumption(assumption, "assumption-form-scope");
  await client.generateDraft(draft, "draft-view-scope");
  assert.deepEqual(calls, [
    { command: "save_usage_plan", args: { payload: { ...plan, expectedScope: "plan-form-scope" } } },
    { command: "set_usage_plan_status", args: { payload: { ...status, expectedScope: "status-view-scope" } } },
    { command: "save_quota_assumption", args: { payload: { ...assumption, expectedScope: "assumption-form-scope" } } },
    { command: "generate_usage_plan_draft", args: { payload: { ...draft, expectedScope: "draft-view-scope" } } },
  ]);
  for (const payload of [plan, status, assumption, draft]) assert.equal("expectedScope" in payload, false, "scope binding must not mutate a retained form payload");
});

test("late overview reads cannot replace an older form's explicitly bound scope", async () => {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  let resolveLate!: (value: SubscriptionOverview) => void;
  let reads = 0;
  const client = createSubscriptionClient(async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "read_subscription_overview") {
      reads++;
      if (reads === 1) return { ...overview(), scopeToken: "opened-form-scope" } as T;
      if (reads === 2) return new Promise<SubscriptionOverview>((resolve) => { resolveLate = resolve; }) as Promise<T>;
      return { ...overview(), scopeToken: "newer-view-scope" } as T;
    }
    return overview() as T;
  });
  const opened = await client.readOverview();
  const input = buildPlanInput({ ...newPlanForm(now), target: "1000" }, opened, null, now);
  const late = client.readOverview();
  assert.equal((await client.readOverview()).scopeToken, "newer-view-scope");
  resolveLate({ ...overview(), scopeToken: "late-read-scope" });
  assert.equal((await late).scopeToken, "late-read-scope");
  await client.savePlan(input, opened.scopeToken);
  assert.deepEqual(calls.at(-1), { command: "save_usage_plan", args: { payload: { ...input, expectedScope: "opened-form-scope" } } });
  await client.savePlan(input);
  assert.deepEqual(calls.at(-1), { command: "save_usage_plan", args: { payload: input } }, "reads must never supply an implicit cached scope");
});

test("historical demand reasons are presented in Chinese with uncertainty intact", () => {
  const codes = ["source_or_runtime_changed", "stable_account_identity_unknown", "scope_unknown", "membership_stale", "entitlement_evidence_unknown", "model_scope_unknown", "initial_clock_unknown", "actual_period_unknown", "source_unknown", "plan_scope_changed", "historical_evidence_changed", "scan_coverage_incomplete", "paired_clock_discontinuous", "entitlement_or_identity_changed", "metering_evidence_unknown", "account_usage_attribution_unknown", "model_coverage_unknown", "view_coverage_unknown", "actual_period_coverage_unknown", "ledger_evidence_unavailable", "history_retention_unknown", "duration_mismatch", "zero_historical_demand_choose_manual_target", "quota_sustainability_unverified", "history_unavailable"];
  for (const code of codes) assert.match(subscriptionReason(code), /[\u3400-\u9fff]/u, code);
  assert.equal(subscriptionReason("zero_historical_demand_choose_manual_target"), "历史需求为零，请手动设置目标");
  assert.equal(subscriptionReason("quota_sustainability_unverified"), "厂商额度能否支撑该需求尚未验证");
});

test("subscription view renders real multi-pool windows and distinguishes all four number sources", () => {
  const value = overview();
  value.plans = [{ ...existingPlan(), used: null, remaining: null, progressPercent: null, evaluationState: "unknown" }];
  value.calibrations = [{ id: "calibration-a", windowId: "shared:primary", model: "actual-model", label: "单模型历史情景", quality: "conditional", capacityLower: null, capacityUpper: null, remainingLower: null, remainingUpper: null, sampleCount: 3, cycleCount: 1, coverage: "用户指定未来区间", structure: "相同 effort 和缓存比例", reasons: ["precision_unknown"] }];
  const html = renderToStaticMarkup(createElement(SubscriptionView, { overview: value, onEditPlan() {}, onToggleReminders() {}, onEndPlan() {} }));
  for (const label of ["实际短窗口", "实际长窗口", "模型附加窗口", "官方额度", "本机模型用量", "我的计划剩余", "条件容量与学习", "等待确认重置", "身份一致性待确认", "系统通知与桌宠提示尚未开放"]) {
    assert.ok(html.includes(label), label);
  }
  assert.ok(html.includes("未知"));
  assert.ok(html.includes("0%"), "recorded zero must remain distinct from missing percentage");
  assert.ok(html.includes("暂不可估"));
  assert.ok(html.includes("假设全部用于 actual-model（与其他模型情景互斥）"));
  assert.ok(html.includes("<details class=\"subscription-scenario\">"), "single-model hypothetical values require an explicit disclosure");
  assert.ok(!html.includes("<details open"));
  assert.ok(!html.includes("面板、收件箱与原生提醒"));
  assert.ok(!html.includes("本周期已记录"), "unconfirmed coverage must use exact ranges");
});

test("privacy-safe summary can retain Core numbers while every editor action is disabled", () => {
  const value = overview();
  value.plans = [{ ...existingPlan(), title: "计划 1" }];
  value.assumptions = [{ id: "assumption-a", revision: 1, sourceId: "source-a", windowId: "shared:primary", model: null, capacityTokens: 2000000, validUntil: "2026-10-23T02:00:00Z", note: "已隐藏" }];
  const html = renderToStaticMarkup(createElement(SubscriptionView, { overview: value, disabled: true, onEditPlan() {}, onToggleReminders() {}, onEndPlan() {}, onEditAssumption() {} }));
  assert.ok(html.includes("25,000 Token"));
  assert.ok(html.includes("计划 1"));
  assert.ok(html.includes("已隐藏"));
  const buttons = html.match(/<button\b[^>]*>/g) ?? [];
  assert.equal(buttons.length, 4);
  assert.ok(buttons.every((button) => button.includes("disabled=")), "masked titles and notes must never be round-tripped by an editor");
});

test("manual remaining scenarios are explicitly selected, conditional and isolated", () => {
  const value = overview();
  value.assumptions = [{ id: "manual-a", revision: 1, sourceId: "source-a", windowId: "shared:primary", model: "A", capacityTokens: 2000000, validUntil: "2026-10-23T02:00:00Z", note: "用户假设", manualScenario: { active: true, provenance: "manual-assumption", quality: "conditional", remainingEstimate: 800000, remainingLower: 790000, remainingUpper: 810000, estimatePrecision: "bounded", constraintsVerified: false, notifyEligible: false, reasons: [] } }];
  const html = renderToStaticMarkup(createElement(SubscriptionView, { overview: value, onEditPlan() {}, onToggleReminders() {}, onEndPlan() {} }));
  assert.ok(html.includes('name="manual-capacity-scenarios"'));
  assert.ok(html.includes("查看此手动假设的余量情景（与其他假设互斥）"));
  assert.ok(html.includes("790,000～810,000 Token"));
  assert.ok(html.includes("不参与自动校准或计划可行性验证"));
  assert.ok(!html.includes("<details open"));
  assert.equal(value.calibrations.length, 0);
  for (const code of ["earlier_related_reset", "manual_assumption_expired", "manual_capacity_invalid", "manual_percent_invalid", "manual_percent_unknown", "manual_percent_saturated", "manual_scope_changed", "manual_scope_unverified", "manual_window_unverified", "usage_structure_unknown"]) assert.notEqual(subscriptionReason(code), code);
});
