import test from "node:test";
import assert from "node:assert/strict";
import { localAmountUnits, normalizeLocalAmount, localScheduleDateInput, localScheduleDateValue, buildLocalScheduleInput, newLocalScheduleForm, buildLocalConsumptionInput, localScheduleFormFromTemplate } from "./localScheduleForms";
import type { LocalScheduleInstance, LocalScheduleTemplate, LocalSchedulesOverview } from "./localScheduleContracts";

test("手动量保留超过 JS 安全整数的精度，credits 固定六位，不接受指数或静默四舍五入", () => {
  assert.equal(normalizeLocalAmount("9007199254740993", "requests"), "9007199254740993");
  assert.equal(localAmountUnits("9007199254740993.000001", "credits"), 9007199254740993000001n);
  assert.equal(normalizeLocalAmount("1.2", "credits"), "1.200000");
  assert.equal(normalizeLocalAmount("0", "credits"), "0.000000");
  for (const text of ["", "1e3", "0x10", "-1", "NaN", "Infinity", "01", "1.0000001", "1,000", "1.", ".5"]) {
    assert.equal(normalizeLocalAmount(text, "credits"), null, text);
  }
  assert.equal(normalizeLocalAmount("1.5", "requests"), null);
  assert.equal(normalizeLocalAmount("9".repeat(65), "tokens"), null);
});

const now = Date.parse("2026-10-23T12:00:00Z");
const overview = (): LocalSchedulesOverview => ({ schemaVersion: 1, revision: 5, scopeToken: "scope-a", asOf: new Date(now).toISOString(), source: { id: "source-a", clientKind: "gemini", enabled: true, configurationRevision: "1" }, templates: [], instances: [], reasons: [] });
const form = () => ({ ...newLocalScheduleForm(now), title: "晚间计划", target: "10000" });
function instance(): LocalScheduleInstance {
  return { id: "instance-a", templateId: "template-a", templateRevision: 1, sourceId: "source-a", title: "手动请求", unit: "requests", target: "100", model: null, modelDimension: "response",
    effectiveFrom: "2026-10-23T10:00:00Z", periodStartsAt: "2026-10-23T00:00:00+08:00", periodEndsAt: "2026-10-24T00:00:00+08:00", endedAt: null,
    lifecycle: "active", evaluationState: "ready", used: "0", actualUsed: null, recordedUsed: "0", remaining: "100", progressPercent: 0, plannedByNow: "10", measurement: "manual-partial", remindersEnabled: false, tokenPlanId: null, reasons: [], entries: [] };
}
test("四种周期统一固定上海时区，续建须显式开启，单次强制 now 且不续建", () => {
  for (const kind of ["once", "daily", "weekly", "monthly"] as const) {
    const f = { ...form(), kind, autoRenew: true, startPolicy: "next_period" as const };
    const input = buildLocalScheduleInput(f, overview(), null, now);
    assert.equal(input.rule.timezone, "+08:00"); assert.equal(input.sourceId, "source-a"); assert.equal(input.expectedScope, "scope-a");
    assert.equal(input.autoRenew, kind !== "once"); assert.equal(input.startPolicy, kind === "once" ? "now" : "next_period");
    assert.equal(input.rule.endsAt === null, kind !== "once");
  }
  assert.equal(buildLocalScheduleInput(form(), overview(), null, now).autoRenew, false);
  const manual = buildLocalScheduleInput({ ...form(), unit: "credits", target: "12.5", model: "pretend", remindersEnabled: true }, overview(), null, now);
  assert.equal(manual.target, "12.500000"); assert.equal(manual.model, null); assert.equal(manual.remindersEnabled, false);
});
test("自由分段按日偏移转换并精确校验权重，拒绝重叠和月末静默截断", () => {
  const f = form(); f.allocation = [{ startDay: "0", startTime: "09:00", endDay: "0", endTime: "12:00", weightPercent: "33.33" }, { startDay: "0", startTime: "13:00", endDay: "0", endTime: "18:00", weightPercent: "66.67" }];
  assert.deepEqual(buildLocalScheduleInput(f, overview(), null, now).allocation, [{ startMinute: 540, endMinute: 720, weightBps: 3333 }, { startMinute: 780, endMinute: 1080, weightBps: 6667 }]);
  const overlap = structuredClone(f); overlap.allocation[1].startTime = "11:00"; assert.throws(() => buildLocalScheduleInput(overlap, overview(), null, now), /重叠/);
  const weight = structuredClone(f); weight.allocation[1].weightPercent = "66.66"; assert.throws(() => buildLocalScheduleInput(weight, overview(), null, now), /100%/);
  const month = { ...form(), kind: "monthly" as const, allocation: [{ startDay: "0", startTime: "00:00", endDay: "29", endTime: "00:00", weightPercent: "100" }] };
  assert.throws(() => buildLocalScheduleInput(month, overview(), null, now), /28 天/);
  month.allocation[0].endDay = "28"; assert.equal(buildLocalScheduleInput(month, overview(), null, now).allocation[0].endMinute, 40320);
});
test("修改只携带原模板 revision 与来源，不更改已实例化快照；开始的单次不编辑", () => {
  const input = buildLocalScheduleInput(form(), overview(), null, now);
  const { expectedScope: _, expectedRevision: __, id: ___, ...base } = input;
  const old: LocalScheduleTemplate = { ...base, id: "template-a", revision: 7, state: "active", createdAt: new Date(now).toISOString(), updatedAt: new Date(now).toISOString(), effectiveFrom: new Date(now).toISOString(), firstStartsAt: new Date(now).toISOString(), configurationRevision: "1", bindingState: "ready", nextPeriodStartsAt: null };
  const snapshot = JSON.stringify(old); const next = buildLocalScheduleInput({ ...form(), target: "20000", unit: "requests" }, overview(), old, now);
  assert.equal(next.id, "template-a"); assert.equal(next.expectedRevision, 7); assert.equal(next.unit, "requests"); assert.equal(JSON.stringify(old), snapshot);
  old.rule.anchorAt = "2026-10-23T10:00:00.123Z";
  const loaded = localScheduleFormFromTemplate(old);
  assert.equal(Date.parse(buildLocalScheduleInput({ ...loaded, target: "20000" }, overview(), old, now).rule.anchorAt), Date.parse(old.rule.anchorAt));
  assert.throws(() => buildLocalScheduleInput(form(), overview(), { ...old, sourceId: "other" }, now), /不属于/);
  const once = { ...old, rule: { ...old.rule, kind: "once" as const, endsAt: "2026-10-24T00:00:00+08:00" } };
  assert.throws(() => buildLocalScheduleInput(form(), overview(), once, now), /不能编辑/);
});
test("手动记录只接受真实区间内时间和正量，原幂等键原样提交，不伪造 Token 或 HTTP 请求", () => {
  const idempotencyKey = "12345678-1234-4123-8123-123456789abc";
  const record = { amount: "3", occurredAt: "2026-10-23T19:00:00", note: "手动填写", idempotencyKey };
  const value = buildLocalConsumptionInput(record, overview(), instance(), now);
  assert.equal(value.idempotencyKey, idempotencyKey); assert.equal(value.amount, "3"); assert.equal(value.occurredAt, "2026-10-23T19:00:00+08:00");
  assert.deepEqual(buildLocalConsumptionInput(record, overview(), instance(), now), value);
  for (const changed of [{ ...record, amount: "0" }, { ...record, amount: "1.1" }, { ...record, occurredAt: "2026-10-23T17:59:59" }, { ...record, occurredAt: "2026-10-24T00:00:00" }, { ...record, occurredAt: "2026-10-23T20:00:01" }]) assert.throws(() => buildLocalConsumptionInput(changed, overview(), instance(), now));
  assert.throws(() => buildLocalConsumptionInput(record, overview(), { ...instance(), unit: "tokens" }, now), /Token/);
  assert.equal(buildLocalConsumptionInput({ ...record, amount: "0.000001" }, overview(), { ...instance(), unit: "credits" }, now).amount, "0.000001");
  assert.throws(() => buildLocalScheduleInput({ ...form(), unit: "credits", target: "9007199254.740992" }, overview(), null, now));
});

test("保存后同一秒立即记录保留毫秒，不把默认发生时间截到实例起点之前", () => {
  const openedAt = Date.parse("2026-10-23T12:00:00.700Z");
  const current = { ...instance(), effectiveFrom: "2026-10-23T12:00:00.500Z" };
  const occurredAt = localScheduleDateInput(new Date(openedAt).toISOString(), "milliseconds");
  assert.equal(occurredAt, "2026-10-23T20:00:00.700");
  const result = buildLocalConsumptionInput({ amount: "1", occurredAt, note: "", idempotencyKey: "12345678-1234-4123-8123-123456789abc" }, overview(), current, openedAt);
  assert.equal(Date.parse(result.occurredAt), openedAt);
  assert.equal(localScheduleDateValue("2026-10-23T20:00:00.7"), "2026-10-23T20:00:00.700+08:00");
});

test("计划日期按固定上海偏移转换，拒绝非法日期和本地时区混用", () => {
  assert.equal(localScheduleDateInput("2026-10-23T00:15:00Z"), "2026-10-23T08:15");
  assert.equal(localScheduleDateValue("2026-10-23T08:15"), "2026-10-23T08:15:00+08:00");
  assert.equal(localScheduleDateValue("2026-02-30T08:15"), null);
  assert.equal(localScheduleDateValue("2026-10-23T24:00"), null);
  assert.equal(localScheduleDateValue("2026-10-23T08:15Z"), null);
  assert.equal(localScheduleDateInput("invalid"), "");
});
