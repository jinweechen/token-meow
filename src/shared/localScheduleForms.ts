/** 本地计划只在表单边界处理十进制字符串，避免金额经过浮点数后丢失精度。 */
import type { LocalScheduleInstance, LocalScheduleKind, LocalScheduleTemplate, LocalSchedulesOverview, LocalScheduleUnit, RecordLocalConsumption, SaveLocalSchedule } from "./localScheduleContracts";
export type { LocalScheduleUnit } from "./localScheduleContracts";

export function localAmountUnits(value: string, unit: LocalScheduleUnit): bigint | null {
  const text = value.trim();
  if (text.length > 64) return null;
  if (unit !== "credits") return /^(?:0|[1-9]\d*)$/.test(text) ? BigInt(text) : null;
  if (!/^(?:0|[1-9]\d*)(?:\.\d{1,6})?$/.test(text)) return null;
  const [integer, fraction = ""] = text.split(".");
  return BigInt(integer) * 1_000_000n + BigInt(fraction.padEnd(6, "0"));
}

export function localAmountText(units: bigint, unit: LocalScheduleUnit): string {
  if (units < 0n) throw Error("记录量不能为负数。");
  return unit === "credits"
    ? `${units / 1_000_000n}.${String(units % 1_000_000n).padStart(6, "0")}`
    : units.toString();
}

export function normalizeLocalAmount(value: string, unit: LocalScheduleUnit): string | null {
  const amount = localAmountUnits(value, unit);
  return amount === null ? null : localAmountText(amount, unit);
}

/** 上海采用固定 +08:00；不能把浏览器所在时区偷偷当作计划时区。 */
export function localScheduleDateInput(value: string | null, precision: boolean | "milliseconds" = false): string {
  if (!value || !Number.isFinite(Date.parse(value))) return "";
  const shifted = new Date(Date.parse(value) + 8 * 60 * 60 * 1000);
  return Number.isFinite(shifted.getTime()) ? shifted.toISOString().slice(0, precision === "milliseconds" ? 23 : precision ? 19 : 16) : "";
}

export function localScheduleDateValue(value: string): string | null {
  const match = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2})(?::([0-5]\d)(?:\.(\d{1,3}))?)?$/.exec(value);
  if (!match) return null;
  const canonical = `${match[1]}:${match[2] ?? "00"}${match[3] ? `.${match[3].padEnd(3, "0")}` : ""}`;
  const result = `${canonical}+08:00`;
  return Number.isFinite(Date.parse(result)) && localScheduleDateInput(result, match[3] ? "milliseconds" : true) === canonical ? result : null;
}

export interface LocalAllocationForm { startDay: string; startTime: string; endDay: string; endTime: string; weightPercent: string }
export interface LocalScheduleForm {
  title: string; unit: LocalScheduleUnit; target: string; kind: LocalScheduleKind; anchorAt: string; endsAt: string;
  startPolicy: "now" | "next_period"; autoRenew: boolean; model: string; modelDimension: "response" | "request";
  allocation: LocalAllocationForm[]; remindersEnabled: boolean;
}
const MAX_AMOUNT = 9007199254740991n;
export function newLocalScheduleForm(now = Date.now()): LocalScheduleForm {
  const current = localScheduleDateInput(new Date(now).toISOString());
  return { title: "", unit: "tokens", target: "", kind: "daily", anchorAt: `${current.slice(0, 10)}T00:00`,
    endsAt: localScheduleDateInput(new Date(now + 2 * 3600000).toISOString()), startPolicy: "now", autoRenew: false,
    model: "", modelDimension: "response", allocation: [], remindersEnabled: true };
}
const minuteTime = (minute: number) => `${String(Math.floor(minute % 1440 / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}`;
export function localScheduleFormFromTemplate(value: LocalScheduleTemplate): LocalScheduleForm {
  return { title: value.title, unit: value.unit, target: value.target, kind: value.rule.kind,
    anchorAt: localScheduleDateInput(value.rule.anchorAt, "milliseconds"), endsAt: localScheduleDateInput(value.rule.endsAt, "milliseconds"),
    startPolicy: value.startPolicy, autoRenew: value.autoRenew, model: value.model ?? "", modelDimension: value.modelDimension,
    allocation: value.allocation.map(part => ({ startDay: String(Math.floor(part.startMinute / 1440)), startTime: minuteTime(part.startMinute),
      endDay: String(Math.floor(part.endMinute / 1440)), endTime: minuteTime(part.endMinute), weightPercent: String(part.weightBps / 100) })),
    remindersEnabled: value.remindersEnabled };
}
function formMinute(day: string, time: string): number {
  if (!/^(?:0|[1-9]\d{0,5})$/.test(day) || !/^(?:[01]\d|2[0-3]):[0-5]\d$/.test(time)) throw Error("分段天数须为非负整数，时间须为有效的 24 小时时间。");
  return Number(day) * 1440 + Number(time.slice(0, 2)) * 60 + Number(time.slice(3));
}
export function buildLocalScheduleInput(form: LocalScheduleForm, overview: LocalSchedulesOverview, existing?: LocalScheduleTemplate | null, now = Date.now()): SaveLocalSchedule {
  if (!overview.source.enabled || !overview.scopeToken) throw Error("请先读取已启用来源的当前计划视图。");
  if (existing && (existing.sourceId !== overview.source.id || existing.state === "disabled")) throw Error("原模板不属于当前来源或已停用，请更新视图。");
  if (existing?.rule.kind === "once" && Date.parse(existing.firstStartsAt) <= now) throw Error("已经开始的单次计划不能编辑，请新建模板。");
  const title = form.title.trim();
  if (!title || title.length > 80 || /[\u0000-\u001f]/.test(title)) throw Error("请填写最多 80 字且不含控制字符的计划名称。");
  const units = localAmountUnits(form.target, form.unit);
  if (units === null || units <= 0n || units > MAX_AMOUNT) throw Error(form.unit === "credits" ? "credits 目标须大于 0、最多六位小数且不超过 9007199254.740991。" : "目标须为 1～9007199254740991 的整数字符串。");
  const anchorAt = localScheduleDateValue(form.anchorAt), endsAt = form.kind === "once" ? localScheduleDateValue(form.endsAt) : null;
  if (!anchorAt) throw Error("请填写有效的周期起点（上海时间）。");
  if (form.kind === "once" && (!endsAt || Date.parse(endsAt) <= now || Date.parse(endsAt) <= Date.parse(anchorAt))) throw Error("单次计划结束时间须晚于周期起点且仍在未来。");
  if (form.allocation.length > 64) throw Error("一个周期最多设置 64 个分段。");
  const maxMinute = form.kind === "daily" ? 1440 : form.kind === "weekly" ? 10080 : form.kind === "monthly" ? 40320 : (Date.parse(endsAt!) - Date.parse(anchorAt)) / 60000;
  const allocation = form.allocation.map(part => {
    const startMinute = formMinute(part.startDay, part.startTime), endMinute = formMinute(part.endDay, part.endTime);
    const weight = part.weightPercent.trim();
    if (!/^(?:0|[1-9]\d{0,2})(?:\.\d{1,2})?$/.test(weight)) throw Error("分段权重须为最多两位小数的百分比。");
    const [whole, fraction = ""] = weight.split("."), weightBps = Number(whole) * 100 + Number(fraction.padEnd(2, "0"));
    if (endMinute <= startMinute || endMinute > maxMinute || weightBps <= 0 || weightBps > 10000) throw Error("分段须在周期内、结束晚于开始且权重大于 0；每月分段最多覆盖前 28 天。");
    return { startMinute, endMinute, weightBps };
  }).sort((a, b) => a.startMinute - b.startMinute);
  if (allocation.length && (!Number.isInteger(maxMinute) || allocation.reduce((n, v) => n + v.weightBps, 0) !== 10000)) throw Error("分段权重合计须为 100%；单次分段要求周期为整分钟。");
  if (allocation.some((v, i) => i > 0 && v.startMinute < allocation[i - 1].endMinute)) throw Error("分段时间不能重叠。");
  const model = form.model.trim() || null;
  if (model && (model.length > 512 || /[\u0000-\u001f]/.test(model) || model === "unknown")) throw Error("模型名称须为明确的精确名称；留空表示该来源全部模型。");
  return { sourceId: overview.source.id, expectedScope: overview.scopeToken, ...(existing ? { id: existing.id } : {}),
    expectedRevision: existing?.revision ?? null, title, unit: form.unit, target: localAmountText(units, form.unit),
    model: form.unit === "tokens" ? model : null, modelDimension: form.modelDimension,
    rule: { kind: form.kind, anchorAt, endsAt, timezone: "+08:00" }, startPolicy: form.kind === "once" ? "now" : form.startPolicy,
    autoRenew: form.kind === "once" ? false : form.autoRenew, allocation, remindersEnabled: form.unit === "tokens" && form.remindersEnabled };
}
export function buildLocalConsumptionInput(form: { amount: string; occurredAt: string; note: string; idempotencyKey: string }, overview: LocalSchedulesOverview, instance: LocalScheduleInstance, now = Date.now()): RecordLocalConsumption {
  if (!overview.source.enabled || instance.sourceId !== overview.source.id || instance.unit === "tokens") throw Error("这里只能记录当前来源的 requests 或 credits；Token 进度由真实账本提供。");
  const units = localAmountUnits(form.amount, instance.unit);
  if (units === null || units <= 0n || units > MAX_AMOUNT) throw Error("记录量须大于 0 且在允许范围内；credits 最多六位小数。");
  const occurredAt = localScheduleDateValue(form.occurredAt), time = occurredAt ? Date.parse(occurredAt) : NaN;
  const end = Math.min(Date.parse(instance.periodEndsAt), instance.endedAt ? Date.parse(instance.endedAt) : Infinity);
  if (!occurredAt || time < Date.parse(instance.effectiveFrom) || time >= end || time > now) throw Error("发生时间须位于此实例实际计量区间内，且不能晚于当前时间。");
  if (form.note.trim().length > 500 || /[\u0000-\u001f]/.test(form.note)) throw Error("记录说明最多 500 字且不能含控制字符。");
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(form.idempotencyKey)) throw Error("记录身份无效，请重新打开记录表单。");
  return { sourceId: overview.source.id, expectedScope: overview.scopeToken, instanceId: instance.id,
    idempotencyKey: form.idempotencyKey, occurredAt, amount: localAmountText(units, instance.unit), note: form.note.trim() };
}
