import test from "node:test";
import assert from "node:assert/strict";
import { parseLocalSchedulesOverview, type LocalSchedulesOverview } from "./localScheduleContracts";

const at = "2026-10-23T12:00:00Z";
function fixture(): LocalSchedulesOverview {
  return { schemaVersion: 1, revision: 1, scopeToken: "scope-a", asOf: at,
    source: { id: "source-a", clientKind: "claude", enabled: true, configurationRevision: "9007199254740993" }, reasons: [],
    templates: [{ id: "template-a", revision: 1, sourceId: "source-a", title: "手动计划", unit: "credits", target: "12.500000", model: null,
      modelDimension: "response", rule: { kind: "daily", anchorAt: "2026-10-23T00:00:00+08:00", endsAt: null, timezone: "+08:00" },
      startPolicy: "now", autoRenew: false, allocation: [], remindersEnabled: false, state: "active", createdAt: at, updatedAt: at,
      effectiveFrom: at, firstStartsAt: at, configurationRevision: "1", bindingState: "ready", nextPeriodStartsAt: null }],
    instances: [{ id: "instance-a", templateId: "template-a", templateRevision: 1, sourceId: "source-a", title: "手动计划", unit: "credits",
      target: "12.500000", model: null, modelDimension: "response", effectiveFrom: at, periodStartsAt: "2026-10-23T00:00:00+08:00",
      periodEndsAt: "2026-10-24T00:00:00+08:00", endedAt: null, lifecycle: "active", evaluationState: "ready", used: "0.000000",
      actualUsed: null, recordedUsed: "0.000000", remaining: "12.500000", progressPercent: 0, plannedByNow: "1.000000",
      measurement: "manual-partial", remindersEnabled: false, tokenPlanId: null, reasons: [], entries: [] }] };
}
test("严格 DTO 保留手动零记录与实际未知，来源配置修订不经过浮点数", () => {
  const value = parseLocalSchedulesOverview(fixture(), "source-a");
  assert.equal(value.source.configurationRevision, "9007199254740993");
  assert.equal(value.instances[0].recordedUsed, "0.000000"); assert.equal(value.instances[0].actualUsed, null);
  const large = fixture(); large.instances[0].used = large.instances[0].recordedUsed = "9007199254740993.000001";
  large.instances[0].progressPercent = 10000; assert.ok(parseLocalSchedulesOverview(large));
});
test("credits 单笔精确微单位边界，累计允许超过单笔上限但不能溢出 i128", () => {
  const value = fixture(); value.templates[0].target = value.instances[0].target = "9007199254.740991";
  assert.ok(parseLocalSchedulesOverview(value));
  for (const target of ["9007199254.740992", "900719925474099.1", "1.2", "1e6", "01.000000", "-1.000000"]) {
    const bad = fixture(); bad.templates[0].target = target; assert.throws(() => parseLocalSchedulesOverview(bad), { name: "Error" }, target);
  }
  const bad = fixture(); bad.instances[0].recordedUsed = "170141183460469231731687303715884105728.000000";
  assert.throws(() => parseLocalSchedulesOverview(bad));
});
test("拒绝跨来源、重复身份、伪造实际消耗、无时区和错误 schema", () => {
  for (const change of [
    (v:any) => v.schemaVersion = 2, (v:any) => v.source.configurationRevision = 2,
    (v:any) => v.templates[0].sourceId = "other", (v:any) => v.instances[0].sourceId = "other",
    (v:any) => v.instances[0].actualUsed = "0.000000", (v:any) => v.instances[0].measurement = "source-recorded",
    (v:any) => v.templates.push(v.templates[0]), (v:any) => v.asOf = "2026-10-23T12:00:00",
    (v:any) => v.templates[0].rule.timezone = "UTC", (v:any) => v.revision = Number.MAX_SAFE_INTEGER + 1,
  ]) { const v = fixture(); change(v); assert.throws(() => parseLocalSchedulesOverview(v)); }
  assert.throws(() => parseLocalSchedulesOverview(fixture(), "other"));
});
test("分段重叠与错误续建拒绝，Token 实例不能带手动条目", () => {
  const bad = fixture(); bad.templates[0].allocation = [{ startMinute: 0, endMinute: 100, weightBps: 5000 }, { startMinute: 50, endMinute: 150, weightBps: 5000 }];
  assert.throws(() => parseLocalSchedulesOverview(bad));
  const once = fixture(); once.templates[0].rule = { ...once.templates[0].rule, kind: "once", endsAt: "2026-10-24T00:00:00+08:00" }; once.templates[0].autoRenew = true;
  assert.throws(() => parseLocalSchedulesOverview(once));
  const token = fixture(); const i = token.instances[0]; i.unit = "tokens"; i.target = "100"; i.used = i.actualUsed = "0"; i.recordedUsed = null; i.remaining = "100"; i.plannedByNow = "20"; i.measurement = "source-recorded"; i.tokenPlanId = "native-plan";
  assert.ok(parseLocalSchedulesOverview(token));
  i.entries = [{ id: "entry", revision: 1, occurredAt: at, createdAt: at, amount: "1", note: "", voided: false }];
  assert.throws(() => parseLocalSchedulesOverview(token));
});
