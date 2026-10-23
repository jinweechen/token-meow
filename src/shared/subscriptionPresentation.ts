import type { LocalPlanForecast, SubscriptionFreshness, SubscriptionQuality, UsagePlan } from "./subscriptionContracts";

const number = new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 2 });
const minute = new Intl.DateTimeFormat("zh-CN", {
  timeZone: "Asia/Shanghai", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false,
});

export function subscriptionNumber(value: number | null | undefined, unit = "") {
  return value == null || !Number.isFinite(value) ? "未知" : `${number.format(value)}${unit}`;
}

export function subscriptionBytes(value: number | null | undefined) {
  if (value == null || !Number.isSafeInteger(value) || value < 0) return "未知";
  if (value < 1024) return `${number.format(value)} B`;
  if (value < 1024 * 1024) return `${number.format(value / 1024)} KiB`;
  return `${number.format(value / (1024 * 1024))} MiB`;
}

export function subscriptionTime(value: string | null | undefined) {
  if (!value) return "未知";
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? minute.format(date) : "未知";
}

export function subscriptionRange(lower: number | null, upper: number | null, unit = " Token") {
  if (lower == null || upper == null || !Number.isFinite(lower) || !Number.isFinite(upper) || lower > upper) return "暂不可估";
  // Never narrow a Core interval through rounding in the view.
  return `${number.format(Math.floor(lower))}～${number.format(Math.ceil(upper))}${unit}`;
}

export function subscriptionPlanForecast(plan: UsagePlan, kind: LocalPlanForecast["kind"]) {
  const forecast = kind === "local_plan_eta" ? plan.forecasts?.localPlanEta : plan.forecasts?.periodEndUsage;
  return plan.type === "tokens" && plan.lifecycle === "active" && plan.evaluationState === "ready"
    && forecast?.kind === kind && forecast.scope === "local-plan"
    && forecast.planId === plan.id && forecast.sourceId === plan.sourceId
    && forecast.modelDimension === plan.modelDimension && forecast.model === plan.model
    && forecast.planConfigurationRevision === plan.revision ? forecast : null;
}

function validTime(value: string | null | undefined) {
  return !!value && Number.isFinite(Date.parse(value));
}

export function subscriptionLocalForecastValue(forecast: LocalPlanForecast | null | undefined) {
  if (!forecast || !validTime(forecast.asOf)) return "暂不可估";
  if (forecast.kind === "local_plan_eta" && forecast.state === "reached") return "目标已达到";
  if (forecast.state !== "conditional") return "暂不可估";
  const range = forecast.empiricalScenarioRange;
  if (!range) return "暂不可估";
  if (forecast.kind === "local_plan_eta") {
    if (!validTime(range.earliestAt) || !validTime(range.latestAt)
      || Date.parse(range.earliestAt!) > Date.parse(range.latestAt!)) return "暂不可估";
    return `${subscriptionTime(range.earliestAt)}～${subscriptionTime(range.latestAt)}`;
  }
  if (range.unit !== "local-plan-tokens" || !Number.isSafeInteger(range.minimum)
    || !Number.isSafeInteger(range.maximum) || range.minimum! < 0 || range.maximum! < 0) return "暂不可估";
  return subscriptionRange(range.minimum!, range.maximum!);
}

export function subscriptionLocalForecastRelation(forecast: LocalPlanForecast | null | undefined) {
  if (!forecast || forecast.kind !== "local_plan_eta" || forecast.state !== "conditional"
    || subscriptionLocalForecastValue(forecast) === "暂不可估") return null;
  if (forecast.relationToPeriodEnd === "before" && forecast.willReachBeforePeriodEnd === true) return "经验情景均在计划截止前达到目标";
  if (forecast.relationToPeriodEnd === "after" && forecast.willReachBeforePeriodEnd === false) return "经验情景均在计划截止后达到目标";
  if (forecast.relationToPeriodEnd === "overlapping" && forecast.willReachBeforePeriodEnd === null) return "经验情景跨过截止时间，是否提前达到目标暂不确定";
  return "当前无法判断是否在截止前达到目标";
}

export const subscriptionFreshness: Record<SubscriptionFreshness, string> = {
  fresh: "最新观测", stale: "已过期", unavailable: "暂不可用",
};
export const subscriptionQuality: Record<SubscriptionQuality, string> = {
  learning: "学习中", conditional: "条件参考", validated: "跨周期验证通过", invalidated: "已失效",
};

const reasons: Record<string, string> = {
  identity_unproven: "身份一致性待确认",
  stale_observation: "当前官方观测已过期",
  recovery_unknown: "恢复规则尚未确认",
  collection_incomplete: "来源采集尚未完整，进度暂不可确认",
  metering_incomplete: "本区间有未知或无效计量，完整进度暂不可确认",
  source_binding_changed: "来源配置已改变，旧实例进度不可继续确认",
  evidence_removed: "部分计量证据已被清理或按保留期移除",
  coverage_unknown: "来源覆盖范围未知",
  usage_conflict_excluded: "冲突用量已隔离，累计量尚待确认",
  window_confirmation_pending: "等待确认绑定窗口或账号连续性",
  source_unavailable: "来源不可用或已停用",
  model_unknown: "来源未提供模型身份",
  model_coverage_incomplete: "部分用量缺少指定模型维度，覆盖不完整",
  native_qualification_pending: "系统通知与桌宠提醒尚未通过原生资格验证",
  comparable_covered_intervals_insufficient: "可比且覆盖可靠的历史区间不足",
  related_pool_constraints_unverified: "其他相关额度池的约束尚未验证",
  earlier_related_reset: "其他相关窗口会更早重置，当前草案暂不能跨越该边界",
  manual_assumption_expired: "手动容量假设已过期",
  manual_assumption_not_started: "手动容量假设尚未生效",
  manual_capacity_invalid: "手动容量数值无效",
  manual_percent_invalid: "当前百分比读数无法用于手动情景",
  manual_percent_unknown: "当前百分比读数未知",
  manual_percent_saturated: "百分比已封顶，剩余余量无法确认",
  manual_scope_changed: "账号、权益、规则或周期已变化，此假设不再适用于当前范围",
  manual_scope_unverified: "手动假设的适用范围尚未验证",
  manual_window_unverified: "手动假设的窗口依据不足",
  usage_structure_unknown: "模型使用结构依据不足",
  source_or_runtime_changed: "来源或当前运行已变化",
  scope_changed: "账号、权益或适用范围已变化，历史结果不再指导当前用量",
  adapter_quota_proof_missing: "厂商读数缺少可验证依据，暂不能自动换算会员 Token 容量",
  stable_account_identity_unknown: "缺少稳定的账号身份依据",
  scope_unknown: "当前数据范围尚未确认",
  membership_stale: "会员观测已过期",
  entitlement_evidence_unknown: "会员权益依据不足",
  model_scope_unknown: "模型范围依据不足",
  initial_clock_unknown: "起始时间依据不足",
  actual_period_unknown: "实际计划区间尚未确认",
  source_unknown: "来源身份未知",
  plan_scope_changed: "计划范围已变化",
  historical_evidence_changed: "历史计量依据已变化",
  scan_coverage_incomplete: "本次扫描覆盖不完整",
  paired_clock_discontinuous: "系统时钟与连续计时不一致",
  entitlement_or_identity_changed: "会员权益或账号身份已变化",
  metering_evidence_unknown: "计量依据不足",
  account_usage_attribution_unknown: "用量归属账号尚未确认",
  model_coverage_unknown: "模型覆盖范围未知",
  view_coverage_unknown: "计划区间覆盖依据不足",
  actual_period_coverage_unknown: "实际计划区间覆盖尚未确认",
  ledger_evidence_unavailable: "本机账本依据暂不可用",
  history_retention_unknown: "历史保留范围尚未确认",
  duration_mismatch: "历史样本实际时长不一致",
  zero_historical_demand_choose_manual_target: "历史需求为零，请手动设置目标",
  quota_sustainability_unverified: "厂商额度能否支撑该需求尚未验证",
  history_unavailable: "历史需求依据暂不可用",
  identity_unknown: "账号身份未知",
  identity_consistency_unproven: "身份一致性待确认",
  identity_consistency_unknown: "身份一致性待确认",
  period_start_unknown: "周期起点未知",
  recovery_semantics_unknown: "恢复规则尚未确认",
  precision_unknown: "百分比精度未知",
  time_basis_unknown: "事实时间依据不足",
  fact_time_unbounded: "事实时间范围无法确认",
  time_intervals_overlap: "采样时间范围重叠",
  observation_out_of_order: "旧观测已隔离",
  not_persisted: "本次观测未持久化",
  insufficient_signal: "有效变化不足",
  insufficient_samples: "有效样本不足",
  external_usage_uncovered: "未覆盖其他设备或云端用量",
  collection_unsettled: "采集尚未结算",
  evidence_evicted: "部分证据已按保留期移除",
  suspected_rule_change: "疑似厂商规则变化",
  constraints_unverified: "额度约束尚未验证",
  waiting_reset: "等待厂商确认重置",
  native_not_qualified: "系统通知与桌宠提醒尚未通过原生资格验证",
  native_notifications_unqualified: "系统通知与桌宠提醒尚未通过原生资格验证",
  local_forecast_scope_unknown: "本机预测的计划范围尚未确认",
  local_forecast_view_mismatch: "预测依据与当前计划进度不一致",
  local_forecast_coverage_unknown: "本机计划采集覆盖尚未确认",
  local_forecast_period_ended: "计划区间已结束，停止预测后续用量",
  local_forecast_usage_unknown: "本机已记录用量未知，暂不能预测",
  local_forecast_target_unknown: "计划目标尚未确认",
  local_forecast_measurement_unknown: "本机用量的计量时间未知",
  local_forecast_measurement_stale: "当前用量计量已过期，等待新扫描",
  local_forecast_clock_discontinuous: "连续采样的时间依据发生变化，正在重新积累",
  local_forecast_scan_unreliable: "本次扫描覆盖不完整，等待可靠采样",
  local_forecast_runtime_changed: "运行环境已变化，正在重新积累近期速度",
  local_forecast_scope_changed: "计划范围已变化，等待当前范围的新采样",
  local_forecast_revision_regressed: "计划修订不连续，等待重新采样",
  local_forecast_usage_reduced: "账本累计用量减少，正在重新确认近期速度",
  local_forecast_samples_insufficient: "连续活动样本不足，暂不能预测",
  local_forecast_measurement_unobserved: "当前用量尚未纳入可靠的速度采样",
  local_forecast_sample_expired: "近期速度样本已过期，等待新扫描",
  local_forecast_recent_change_missing: "近期缺少持续用量变化，暂不能外推速度",
  local_forecast_numeric_overflow: "预测超出可安全展示的数值范围",
  local_forecast_ledger_changed: "本机计量依据已变化，等待重新确认近期速度",
  projection_budget_exceeded: "部分配对超出本次计算预算，容量资格暂不可确认；原始证据仍保留",
};

export function subscriptionReason(value: string) {
  return reasons[value] ?? value;
}

export function subscriptionCoverage(value: string) {
  const labels: Record<string, string> = { "recorded-local": "本机已记录，非全账号覆盖", unknown: "覆盖未知", partial: "部分覆盖", verified: "已验证覆盖", "user-specified": "用户指定范围" };
  return labels[value] ?? value;
}

export function subscriptionProvenance(value: string) {
  const labels: Record<string, string> = { "official-observation": "厂商观测", official: "厂商观测", manual: "用户补充", "manual-assumption": "手动假设" };
  return labels[value] ?? value;
}

export function subscriptionModelName(value: string | null) {
  return !value || value === "unknown" ? "未知模型" : value;
}
