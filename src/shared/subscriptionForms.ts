import type { QuotaAssumption, SaveQuotaAssumptionInput, SaveUsagePlanInput, SubscriptionOverview, UsagePlan } from "./subscriptionContracts";

export interface PlanForm {
  title: string;
  type: "tokens" | "percent";
  model: string;
  modelDimension: "response" | "request";
  target: string;
  endPolicy: "time" | "window";
  endsAt: string;
  windowId: string;
  remindersEnabled: boolean;
}

/** datetime-local input is explicitly Shanghai time, independent of OS time zone. */
export function subscriptionDateInput(value: string | null) {
  if (!value || !Number.isFinite(Date.parse(value))) return "";
  return new Date(Date.parse(value) + 8 * 60 * 60 * 1000).toISOString().slice(0, 16);
}

export function subscriptionDateValue(value: string) {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(value)) return null;
  const date = new Date(`${value}:00+08:00`);
  if (!Number.isFinite(date.getTime()) || subscriptionDateInput(date.toISOString()) !== value) return null;
  return date.toISOString();
}

export function newPlanForm(now = Date.now()): PlanForm {
  return {
    title: "", type: "tokens", model: "", modelDimension: "response", target: "",
    endPolicy: "time", endsAt: subscriptionDateInput(new Date(now + 2 * 60 * 60 * 1000).toISOString()),
    windowId: "", remindersEnabled: true,
  };
}

export function planFormFromInput(input: SaveUsagePlanInput): PlanForm {
  return {
    title: input.title, type: input.type, model: input.model ?? "", modelDimension: input.modelDimension,
    target: String(input.target), endPolicy: input.endPolicy, endsAt: subscriptionDateInput(input.endsAt),
    windowId: input.windowId ?? "", remindersEnabled: input.remindersEnabled,
  };
}

export function buildPlanInput(form: PlanForm, overview: SubscriptionOverview, existing?: UsagePlan | null, now = Date.now()): SaveUsagePlanInput {
  if (!overview.source || !overview.capabilities.manualPlans) throw Error("当前来源暂不支持保存计划。");
  const target = Number(form.target);
  const type = existing?.type ?? form.type;
  if (!form.target.trim() || !Number.isFinite(target) || target <= 0 || (type === "tokens" && !Number.isSafeInteger(target)) || (type === "percent" && target > 100)) {
    throw Error(type === "tokens" ? "Token 目标须为大于 0 的安全整数。" : "官方已用上限须大于 0 且不超过 100%。");
  }
  const title = form.title.trim() || (type === "tokens" ? "本机 Token 计划" : "官方比例目标");
  if (title.length > 80) throw Error("计划名称最多 80 个字符。");
  // A goal revision must retain the server's original immutable counting boundary.
  if (existing) {
    if (existing.lifecycle !== "active" || existing.sourceId !== overview.source.id) throw Error("该计划已结束或不属于当前来源，请刷新后重试。");
    return { id: existing.id, expectedRevision: existing.revision, title, type: existing.type, sourceId: existing.sourceId,
      model: existing.model, modelDimension: existing.modelDimension, target, endPolicy: existing.endPolicy,
      endsAt: existing.endsAt, windowId: existing.windowId, remindersEnabled: form.remindersEnabled };
  }
  const endPolicy = type === "percent" ? "window" : form.endPolicy;
  const window = overview.windows.find((value) => value.id === form.windowId);
  if (endPolicy === "window" && (!window?.canBindPlan || window.resetState !== "current")) throw Error("请选择可绑定的当前官方窗口；待确认或过期窗口不能开始计划。");
  const endsAt = endPolicy === "time" ? subscriptionDateValue(form.endsAt) : null;
  if (endPolicy === "time" && (!endsAt || Date.parse(endsAt) <= now)) throw Error("结束时间须为未来的有效上海时间。");
  const model = type === "tokens" ? form.model.trim() || null : null;
  if (model && (model === "unknown" || !overview.modelUsage.some((value) => value.model === model && value.modelDimension === form.modelDimension))) throw Error("请选择当前来源实际记录的模型与维度。");
  return { expectedRevision: null, title, type, sourceId: overview.source.id, model,
    modelDimension: form.modelDimension, target, endPolicy, endsAt, windowId: endPolicy === "window" ? form.windowId : null,
    remindersEnabled: form.remindersEnabled };
}

export function buildAssumptionInput(form: { windowId: string; model: string; capacityTokens: string; validUntil: string; note: string }, overview: SubscriptionOverview, now = Date.now(), existing?: QuotaAssumption | null): SaveQuotaAssumptionInput {
  if (!overview.source || !overview.capabilities.capacityAssumptions) throw Error("当前来源暂不支持容量依据。");
  if (!overview.windows.some((window) => window.id === form.windowId)) throw Error("请选择容量假设对应的官方窗口。");
  const capacityTokens = Number(form.capacityTokens);
  if (!form.capacityTokens.trim() || !Number.isSafeInteger(capacityTokens) || capacityTokens <= 0) throw Error("手动容量须为大于 0 的安全整数。");
  const validUntil = subscriptionDateValue(form.validUntil);
  if (!validUntil || Date.parse(validUntil) <= now) throw Error("请填写未来的假设失效时间。");
  if (!form.note.trim() || form.note.length > 500) throw Error("请说明容量依据和使用结构，最多 500 个字符。");
  const model = form.model.trim() || null;
  if (model && (model === "unknown" || !overview.modelUsage.some((usage) => usage.model === model))) throw Error("请选择已记录模型，或使用当前模型组合。");
  if (existing && existing.sourceId !== overview.source.id) throw Error("该容量依据不属于当前来源，请刷新后重试。");
  return { ...(existing ? { id: existing.id } : {}), expectedRevision: existing?.revision ?? null, sourceId: overview.source.id, windowId: form.windowId, model,
    capacityTokens, validUntil, note: form.note.trim() };
}
