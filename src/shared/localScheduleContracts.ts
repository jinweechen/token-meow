export type LocalScheduleUnit = "tokens" | "requests" | "credits";
export type LocalScheduleKind = "once" | "daily" | "weekly" | "monthly";
export type LocalScheduleState = "active" | "paused" | "disabled";
export interface LocalScheduleRule { kind: LocalScheduleKind; anchorAt: string; endsAt: string | null; timezone: "+08:00" }
export interface LocalScheduleAllocation { startMinute: number; endMinute: number; weightBps: number }
export interface SaveLocalSchedule {
  sourceId: string; expectedScope: string; id?: string; expectedRevision: number | null;
  title: string; unit: LocalScheduleUnit; model: string | null; modelDimension: "response" | "request";
  rule: LocalScheduleRule; startPolicy: "now" | "next_period"; target: string; autoRenew: boolean;
  allocation: LocalScheduleAllocation[]; remindersEnabled: boolean;
}
export interface LocalScheduleTemplate extends Omit<SaveLocalSchedule, "expectedScope" | "expectedRevision" | "id"> {
  id: string; revision: number; state: LocalScheduleState; createdAt: string; updatedAt: string;
  effectiveFrom: string; firstStartsAt: string; configurationRevision: string;
  bindingState: "ready" | "pending_rebind" | "source_unavailable"; nextPeriodStartsAt: string | null;
}
export interface LocalConsumptionEntry { id: string; revision: number; occurredAt: string; amount: string; note: string; createdAt: string; voided: boolean }
export interface LocalScheduleInstance {
  id: string; templateId: string; templateRevision: number; sourceId: string; title: string; unit: LocalScheduleUnit;
  target: string; model: string | null; modelDimension: "response" | "request"; effectiveFrom: string;
  periodStartsAt: string; periodEndsAt: string; endedAt: string | null; lifecycle: "active" | "ended";
  evaluationState: "ready" | "pending" | "unknown"; used: string | null; actualUsed: string | null;
  recordedUsed: string | null; remaining: string | null; progressPercent: number | null; plannedByNow: string;
  measurement: "source-recorded" | "manual-partial"; remindersEnabled: boolean; tokenPlanId: string | null;
  reasons: string[]; entries: LocalConsumptionEntry[];
}
export interface LocalSchedulesOverview {
  schemaVersion: 1; revision: number; scopeToken: string; asOf: string;
  source: { id: string; clientKind: string; enabled: boolean; configurationRevision: string };
  templates: LocalScheduleTemplate[]; instances: LocalScheduleInstance[]; reasons: string[];
}
export interface SetLocalScheduleStatus { sourceId: string; expectedScope: string; id: string; expectedRevision: number; state: LocalScheduleState }
export interface RecordLocalConsumption { sourceId: string; expectedScope: string; instanceId: string; idempotencyKey: string; occurredAt: string; amount: string; note: string }
export interface VoidLocalConsumption { sourceId: string; expectedScope: string; entryId: string; idempotencyKey: string; expectedRevision: number }

type Obj = Record<string, unknown>;
const obj = (v: unknown): v is Obj => !!v && typeof v === "object" && !Array.isArray(v);
const text = (v: unknown, max = 512): v is string => typeof v === "string" && v.length > 0 && v.length <= max && !/[\u0000-\u001f]/.test(v);
const integer = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const date = (v: unknown): v is string => typeof v === "string" && v.length <= 64 && /^\d{4}-\d\d-\d\dT.*(?:Z|[+-]\d\d:\d\d)$/.test(v) && Number.isFinite(Date.parse(v));
const decimal = (v: unknown): v is string => typeof v === "string" && /^(?:0|[1-9]\d{0,19})$/.test(v) && BigInt(v) <= 18446744073709551615n;
const one = <T extends string>(v: unknown, values: readonly T[]): v is T => typeof v === "string" && values.includes(v as T);
const strings = (v: unknown): v is string[] => Array.isArray(v) && v.length <= 128 && v.every(x => text(x, 2048));
const maybeText = (v: unknown): v is string | null => v === null || text(v);
function amount(v: unknown, unit: LocalScheduleUnit, positive = false, capped = false): v is string {
  if (typeof v !== "string" || v.length > 48 || !(unit === "credits" ? /^(?:0|[1-9]\d*)\.\d{6}$/ : /^(?:0|[1-9]\d*)$/).test(v)) return false;
  const [whole, fraction = ""] = v.split(".");
  const units = unit === "credits" ? BigInt(whole) * 1_000_000n + BigInt(fraction.padEnd(6, "0")) : BigInt(whole);
  return units <= (capped ? 9007199254740991n : 170141183460469231731687303715884105727n) && (!positive || units > 0n);
}
function list<T>(value: unknown, read: (v: unknown) => T): T[] {
  if (!Array.isArray(value) || value.length > 10_000) throw Error("本地计划列表格式无效。");
  return value.map(read);
}
function rule(value: unknown): LocalScheduleRule {
  if (!obj(value) || !one(value.kind, ["once", "daily", "weekly", "monthly"] as const) || !date(value.anchorAt)
      || value.timezone !== "+08:00" || !(value.endsAt === null || date(value.endsAt))
      || (value.kind === "once" ? value.endsAt === null || Date.parse(value.endsAt) <= Date.parse(value.anchorAt) : value.endsAt !== null))
    throw Error("本地计划周期格式无效。");
  return { kind: value.kind, anchorAt: value.anchorAt, endsAt: value.endsAt, timezone: "+08:00" };
}
function template(value: unknown, sourceId: string): LocalScheduleTemplate {
  if (!obj(value) || !text(value.id) || value.sourceId !== sourceId || !integer(value.revision) || !text(value.title, 80)
      || !one(value.unit, ["tokens", "requests", "credits"] as const) || !amount(value.target, value.unit, true, true)
      || !maybeText(value.model) || !one(value.modelDimension, ["response", "request"] as const)
      || !one(value.startPolicy, ["now", "next_period"] as const) || typeof value.autoRenew !== "boolean" || typeof value.remindersEnabled !== "boolean"
      || !one(value.state, ["active", "paused", "disabled"] as const) || !date(value.createdAt) || !date(value.updatedAt)
      || !date(value.effectiveFrom) || !date(value.firstStartsAt) || !decimal(value.configurationRevision)
      || !one(value.bindingState, ["ready", "pending_rebind", "source_unavailable"] as const)
      || !(value.nextPeriodStartsAt === null || date(value.nextPeriodStartsAt))) throw Error("本地计划模板格式无效。");
  const scheduleRule = rule(value.rule);
  const allocation = list(value.allocation, v => {
    if (!obj(v) || !integer(v.startMinute) || !integer(v.endMinute) || !integer(v.weightBps) || v.endMinute <= v.startMinute || v.weightBps > 10000)
      throw Error("本地计划分段格式无效。");
    return { startMinute: v.startMinute, endMinute: v.endMinute, weightBps: v.weightBps };
  });
  if (allocation.length > 64 || allocation.some((v, i) => i > 0 && v.startMinute < allocation[i - 1].endMinute)
      || (allocation.length > 0 && allocation.reduce((n, v) => n + v.weightBps, 0) !== 10000)
      || (scheduleRule.kind === "once" && (value.autoRenew || value.startPolicy !== "now"))) throw Error("本地计划分段或续建条件无效。");
  return { id: value.id, revision: value.revision, sourceId, title: value.title, unit: value.unit, target: value.target,
    model: value.model, modelDimension: value.modelDimension, rule: scheduleRule, startPolicy: value.startPolicy,
    autoRenew: value.autoRenew, remindersEnabled: value.remindersEnabled, allocation, state: value.state,
    createdAt: value.createdAt, updatedAt: value.updatedAt, effectiveFrom: value.effectiveFrom, firstStartsAt: value.firstStartsAt,
    configurationRevision: value.configurationRevision, bindingState: value.bindingState, nextPeriodStartsAt: value.nextPeriodStartsAt };
}
function instance(value: unknown, sourceId: string): LocalScheduleInstance {
  if (!obj(value) || !text(value.id) || !text(value.templateId) || !integer(value.templateRevision) || value.sourceId !== sourceId
      || !text(value.title, 80) || !one(value.unit, ["tokens", "requests", "credits"] as const) || !amount(value.target, value.unit, true, true)
      || !maybeText(value.model) || !one(value.modelDimension, ["response", "request"] as const) || !date(value.effectiveFrom)
      || !date(value.periodStartsAt) || !date(value.periodEndsAt) || Date.parse(value.periodEndsAt) <= Date.parse(value.periodStartsAt)
      || !(value.endedAt === null || date(value.endedAt)) || !one(value.lifecycle, ["active", "ended"] as const)
      || !one(value.evaluationState, ["ready", "pending", "unknown"] as const) || typeof value.remindersEnabled !== "boolean"
      || !maybeText(value.tokenPlanId) || !strings(value.reasons) || !one(value.measurement, ["source-recorded", "manual-partial"] as const)
      || !amount(value.plannedByNow, value.unit)) throw Error("本地计划实例格式无效。");
  for (const key of ["used", "actualUsed", "recordedUsed", "remaining"] as const) {
    if (value[key] !== null && !amount(value[key], value.unit)) throw Error("本地计划计量值无效。");
  }
  if (value.progressPercent !== null && !(typeof value.progressPercent === "number" && Number.isFinite(value.progressPercent) && value.progressPercent >= 0))
    throw Error("本地计划进度无效。");
  if (value.unit !== "tokens" && (value.actualUsed !== null || value.measurement !== "manual-partial" || value.tokenPlanId !== null))
    throw Error("手动记录不能充当来源实际消耗。");
  const entries = list(value.entries, v => {
    if (!obj(v) || !text(v.id) || !integer(v.revision) || !date(v.occurredAt) || !date(v.createdAt) || !amount(v.amount, value.unit as LocalScheduleUnit, true, true)
        || typeof v.note !== "string" || v.note.length > 500 || typeof v.voided !== "boolean") throw Error("手动消耗记录格式无效。");
    return { id: v.id, revision: v.revision, occurredAt: v.occurredAt, createdAt: v.createdAt, amount: v.amount, note: v.note, voided: v.voided };
  });
  if (value.unit === "tokens" && (value.measurement !== "source-recorded" || entries.length !== 0)) throw Error("Token 计划必须来自真实账本投影。");
  return { id: value.id, templateId: value.templateId, templateRevision: value.templateRevision, sourceId,
    title: value.title, unit: value.unit, target: value.target, model: value.model, modelDimension: value.modelDimension,
    effectiveFrom: value.effectiveFrom, periodStartsAt: value.periodStartsAt, periodEndsAt: value.periodEndsAt,
    endedAt: value.endedAt, lifecycle: value.lifecycle, evaluationState: value.evaluationState,
    used: value.used as string | null, actualUsed: value.actualUsed as string | null, recordedUsed: value.recordedUsed as string | null,
    remaining: value.remaining as string | null, progressPercent: value.progressPercent as number | null, plannedByNow: value.plannedByNow,
    measurement: value.measurement, remindersEnabled: value.remindersEnabled, tokenPlanId: value.tokenPlanId, reasons: value.reasons, entries };
}
export function parseLocalSchedulesOverview(value: unknown, sourceId?: string): LocalSchedulesOverview {
  if (!obj(value) || value.schemaVersion !== 1 || !integer(value.revision) || !text(value.scopeToken, 2048) || !date(value.asOf)
      || !obj(value.source) || !text(value.source.id) || (sourceId !== undefined && value.source.id !== sourceId)
      || !text(value.source.clientKind) || typeof value.source.enabled !== "boolean" || !decimal(value.source.configurationRevision) || !strings(value.reasons))
    throw Error("本地计划概览格式或来源无效。");
  const id = value.source.id;
  const templates = list(value.templates, v => template(v, id)), instances = list(value.instances, v => instance(v, id));
  for (const rows of [templates, instances, ...instances.map(i => i.entries)]) {
    if (new Set(rows.map(v => v.id)).size !== rows.length) throw Error("本地计划记录身份重复。");
  }
  return { schemaVersion: 1, revision: value.revision, scopeToken: value.scopeToken, asOf: value.asOf,
    source: { id, clientKind: value.source.clientKind, enabled: value.source.enabled, configurationRevision: value.source.configurationRevision },
    templates, instances, reasons: value.reasons };
}
