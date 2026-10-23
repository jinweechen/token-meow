import type { QuotaAssumption, QuotaCalibration, SubscriptionOverview, SubscriptionWindow, UsagePlan, UsagePlanDraft } from "../../shared/subscriptionContracts";
import { subscriptionBytes, subscriptionCoverage, subscriptionFreshness, subscriptionLocalForecastRelation, subscriptionLocalForecastValue, subscriptionModelName, subscriptionNumber, subscriptionPlanForecast, subscriptionProvenance, subscriptionQuality, subscriptionRange, subscriptionReason, subscriptionTime } from "../../shared/subscriptionPresentation";

export function SubscriptionReasons({ values }: { values: string[] }) {
  return values.length ? <ul className="subscription-reasons">{[...new Set(values)].map((reason) => <li key={reason}>{subscriptionReason(reason)}</li>)}</ul> : null;
}

function CapacityCard({ calibration }: { calibration: QuotaCalibration }) {
  const point = calibration.empiricalCapacity;
  const empirical = calibration.quality === "conditional" && calibration.reasons.includes("precision_unknown")
    && calibration.capacityLower == null && calibration.capacityUpper == null
    && calibration.remainingLower == null && calibration.remainingUpper == null
    && point != null && Number.isFinite(point) && point > 0 && point <= Number.MAX_SAFE_INTEGER;
  return <article className="subscription-card"><div className="subscription-section-heading"><h4>{calibration.label}</h4><span className="subscription-tag">{subscriptionQuality[calibration.quality]}</span></div><p className="subscription-note">{calibration.model ? `假设全部用于 ${calibration.model}（互斥情景）` : "当前模型组合"}</p>{calibration.quality === "invalidated" && <p className="subscription-note">以下仅为已失效的历史依据，不再指导新计划。</p>}<dl className="subscription-capacity-values"><div><dt>完整窗口等效容量</dt><dd>{subscriptionRange(calibration.capacityLower, calibration.capacityUpper)}</dd></div><div><dt>继续相同结构的剩余情景</dt><dd>{subscriptionRange(calibration.remainingLower, calibration.remainingUpper)}</dd></div>{empirical && <div><dt>精度未知的经验情景</dt><dd>约 {subscriptionNumber(point, " Token")}</dd></div>}</dl>{empirical && <p className="subscription-note">仅为继续相同使用结构时的完整窗口经验点；百分比精度未知，误差区间、剩余 Token 和耗尽时间仍不可估。此情景不参与计划可持续性验证或提醒。</p>}<p className="subscription-note">有效配对 {calibration.sampleCount} · 确认周期 {calibration.cycleCount} · {subscriptionCoverage(calibration.coverage)}</p><p className="subscription-note">使用结构：{calibration.structure}</p><SubscriptionReasons values={calibration.reasons} /></article>;
}

function ManualScenario({ assumption }: { assumption: QuotaAssumption }) {
  const scenario = assumption.manualScenario;
  return <details className="subscription-scenario" name="manual-capacity-scenarios"><summary>查看此手动假设的余量情景（与其他假设互斥）</summary><p className="subscription-note">用户容量假设 · 不参与自动校准或计划可行性验证。</p>{scenario?.estimatePrecision === "bounded" ? <p>手动条件余量：{subscriptionRange(scenario.remainingLower, scenario.remainingUpper)}</p> : <p>手动条件参考：{subscriptionNumber(scenario?.remainingEstimate, " Token")}（百分比精度未证明，区间未知）</p>}<SubscriptionReasons values={scenario?.reasons ?? ["manual_scope_unverified"]} /></details>;
}

function LocalPlanScenarios({ plan }: { plan: UsagePlan }) {
  const eta = subscriptionPlanForecast(plan, "local_plan_eta");
  const periodEnd = subscriptionPlanForecast(plan, "period_end_usage");
  const relation = subscriptionLocalForecastRelation(eta);
  const asOf = eta?.asOf ?? periodEnd?.asOf;
  const reasons = [...(eta?.reasons ?? []), ...(periodEnd?.reasons ?? [])];
  return <div className="subscription-plan-forecast" aria-label="本机计划用量预测">
    <strong>本机用量情景 · 实验</strong>
    <dl className="subscription-forecast-values"><div data-forecast-kind="local_plan_eta"><dt>本机目标耗尽情景（上海时区）</dt><dd>{subscriptionLocalForecastValue(eta)}</dd></div><div data-forecast-kind="period_end_usage"><dt>计划截止时本机累计用量情景</dt><dd>{subscriptionLocalForecastValue(periodEnd)}</dd></div></dl>
    {relation && <p className="subscription-note">{relation}</p>}
    <p className="subscription-note">按最近本机活动速度继续使用的经验情景，不是保证或置信区间；本机目标与厂商剩余额度分别计算。</p>
    {asOf && <p className="subscription-note">采样基准 {subscriptionTime(asOf)} · 上海时区</p>}
    {!eta && !periodEnd && <p className="subscription-note">等待当前计划的可靠近期采样。</p>}
    <SubscriptionReasons values={reasons} />
  </div>;
}

export function UsagePlanDraftBasis({ draft }: { draft: UsagePlanDraft }) {
  return <div className="subscription-draft-basis"><h3>{draft.kind === "historical_demand" ? "历史需求草案" : "草案依据"}</h3>
    {draft.kind === "historical_demand" && <p>历史 P50 用于建议本机目标，P90 用于高需求参考；它们描述历史需求，厂商额度可持续性仍待验证。</p>}
    <p>{draft.constraintsVerified ? "Core 已验证本次草案的可计算约束；仍须编辑并保存后启用。" : "额度约束未验证；这是可编辑的需求或原生目标建议，不保证厂商允许相应用量。"}</p>
    {draft.basis.length > 0 && <ul>{draft.basis.map((basis, index) => <li key={index}>{basis}</li>)}</ul>}<SubscriptionReasons values={draft.reasons} />{!draft.plan && <p>当前数据不足以生成草案，可直接新建手动计划。</p>}
  </div>;
}

function EvidenceAvailability({ overview }: { overview: SubscriptionOverview }) {
  const evidence = overview.evidenceAvailability;
  if (!evidence || !overview.source || evidence.sourceId !== overview.source.id) return null;
  const truncation = evidence.capacityTruncation;
  const capacityDeleted = truncation.deletedBySourceCount + truncation.deletedBySourceBytes + truncation.deletedByGlobalBytes;
  return <details className="subscription-scenario subscription-evidence"><summary>证据保留范围与容量</summary><div className="subscription-evidence-content">
    <dl className="subscription-capacity-values"><div><dt>最早可用证据的本机采集时间（上海时区）</dt><dd>{subscriptionTime(evidence.earliestAvailableAt)}</dd></div><div><dt>最早明确记载的厂商事实时间（上海时区）</dt><dd>{subscriptionTime(evidence.earliestRecordedFactAt)}</dd></div><div><dt>已保留观测</dt><dd>{subscriptionNumber(evidence.receiptCount, " 条")}</dd></div><div><dt>当前保存内容量</dt><dd>{subscriptionBytes(evidence.payloadBytes)}</dd></div></dl>
    <p className="subscription-note">证据保留上限 {subscriptionNumber(evidence.retentionDays, " 天")}；单来源最多 {subscriptionNumber(evidence.limits.perSourceReceipts, " 条")} / {subscriptionBytes(evidence.limits.perSourcePayloadBytes)}，全部来源内容量上限 {subscriptionBytes(evidence.limits.totalPayloadBytes)}。</p>
    <p className="subscription-note">采集时间与厂商事实时间分别记录。保存内容量统计观测内容，不等于数据库文件的磁盘占用。</p>
    {truncation.lastAppliedAt && <p className="subscription-note">最近一次全局订阅证据清理 {subscriptionTime(truncation.lastAppliedAt)}：按保留期移除 {subscriptionNumber(truncation.deletedByAge, " 条")}，按容量移除 {subscriptionNumber(capacityDeleted, " 条")}。</p>}
    {evidence.unresolvedReferenceCount > 0 && <p className="subscription-note">{subscriptionNumber(evidence.unresolvedReferenceCount, " 条")}观测缺少原始依据，不能作为可用证据。</p>}
    <p className="subscription-note">最早可用范围会随保留期和容量清理变化；已移除的依据不会继续支持预测。</p>
  </div></details>;
}

function PlanCard({ plan, window, disabled, onEdit, onReminders, onEnd }: {
  plan: UsagePlan; window?: SubscriptionWindow; disabled: boolean;
  onEdit: (plan: UsagePlan) => void; onReminders: (plan: UsagePlan) => void; onEnd: (plan: UsagePlan) => void;
}) {
  const unit = plan.type === "tokens" ? " Token" : "%";
  return <article className="subscription-card subscription-plan-card">
    <div className="subscription-section-heading"><h4>{plan.title}</h4><span className="subscription-tag">{plan.lifecycle === "ended" ? "已结束" : plan.evaluationState === "ready" ? "执行中" : plan.evaluationState === "pending" ? "等待确认" : "进度未知"}</span></div>
    <p className="subscription-note">{plan.type === "tokens" ? "来源本机预算" : "官方窗口目标"} · {plan.model ?? "此来源全部模型"}{plan.model ? ` · ${plan.modelDimension === "response" ? "响应模型" : "请求模型"}` : ""}</p>
    <dl className="subscription-amounts"><div><dt>{plan.type === "tokens" ? "本机已记录" : "官方当前已用"}</dt><dd>{subscriptionNumber(plan.used, unit)}</dd></div><div><dt>我的计划目标</dt><dd>{subscriptionNumber(plan.target, unit)}</dd></div><div><dt>我的计划剩余</dt><dd>{subscriptionNumber(plan.remaining, plan.type === "tokens" ? " Token" : " 个百分点")}</dd></div>{plan.windowId && <div><dt>绑定窗口官方剩余</dt><dd>{subscriptionNumber(window?.remainingPercent, "%")}</dd></div>}</dl>
    <p className="subscription-plan-progress">计划进度 <strong>{subscriptionNumber(plan.progressPercent, "%")}</strong></p>
    <p className="subscription-note">{subscriptionTime(plan.effectiveFrom)} 起 · {plan.endPolicy === "window" ? "绑定窗口确认结束" : `截止 ${subscriptionTime(plan.endsAt)}`}{plan.endPolicy === "window" && plan.endsAt ? `（当前依据 ${subscriptionTime(plan.endsAt)}）` : ""}</p>
    <p className="subscription-note">{plan.remindersEnabled ? "提醒已启用" : "提醒已暂停，仍持续计量"} · 配置修订 {plan.revision}</p>
    <SubscriptionReasons values={plan.reasons} />
    {plan.type === "tokens" && plan.lifecycle === "active" && <LocalPlanScenarios plan={plan} />}
    {plan.lifecycle === "active" && <div className="subscription-inline-actions"><button type="button" disabled={disabled} onClick={() => onEdit(plan)}>修改目标</button><button type="button" disabled={disabled} onClick={() => onReminders(plan)}>{plan.remindersEnabled ? "暂停提醒" : "恢复提醒"}</button><button type="button" disabled={disabled} onClick={() => onEnd(plan)}>结束计划</button></div>}
  </article>;
}

export function SubscriptionView({ overview, disabled = false, onEditPlan, onToggleReminders, onEndPlan, onEditAssumption }: {
  overview: SubscriptionOverview; disabled?: boolean;
  onEditPlan: (plan: UsagePlan) => void; onToggleReminders: (plan: UsagePlan) => void; onEndPlan: (plan: UsagePlan) => void;
  onEditAssumption?: (assumption: QuotaAssumption) => void;
}) {
  const buckets = new Map<string, typeof overview.windows>();
  for (const window of overview.windows) buckets.set(window.bucketId, [...(buckets.get(window.bucketId) ?? []), window]);
  return <>
    <div className="subscription-summary-grid">
      <article className="subscription-card"><h3>会员与来源</h3><strong className="subscription-main-value">{overview.membership?.label ?? "会员信息未知"}</strong><p>{overview.source?.label ?? "尚未选择来源"}</p>{overview.membership && <p className="subscription-note">{overview.membership.subTier ?? "具体档位未提供"} · {subscriptionFreshness[overview.membership.freshness]} · {subscriptionProvenance(overview.membership.source)}</p>}<SubscriptionReasons values={overview.membership?.reasons ?? []} /></article>
      <article className="subscription-card"><h3>提醒通道</h3><strong className="subscription-main-value">{overview.notifications.channel === "native" && overview.notifications.qualified && overview.capabilities.nativeNotifications ? "面板、收件箱与原生提醒" : "面板与收件箱"}</strong><p className="subscription-note">{overview.capabilities.nativeNotifications ? "系统通知和桌宠提示仍受静音、免打扰及宿主许可约束。" : "系统通知与桌宠提示尚未开放。"}</p><SubscriptionReasons values={overview.notifications.reasons} /></article>
    </div>
    <SubscriptionReasons values={overview.reasons} />
    <section className="subscription-section" aria-labelledby="subscription-official-title"><div className="subscription-section-heading"><h3 id="subscription-official-title">官方额度</h3><span className="subscription-tag">厂商原生百分比</span></div><p className="subscription-note">每个共享池与窗口分别展示；窗口不能相加，付款周期不等于额度周期。</p>
      {!overview.capabilities.officialQuota && <p className="subscription-empty">账号与额度能力尚未通过当前来源的证据门；独立本机计划仍可使用。</p>}
      {!overview.windows.length && <p className="subscription-empty">当前没有可展示的官方窗口；不会补出默认五小时或周额度。</p>}
      {[...buckets.entries()].map(([bucketId, windows]) => <div key={bucketId} className="subscription-bucket"><h4>{windows[0].bucketName} <span>额度池</span></h4><div className="subscription-window-grid">{windows.map((window) => <article className="subscription-card" key={window.id}>
        <div className="subscription-section-heading"><h4>{window.label}</h4><span className="subscription-tag">{window.resetState === "waiting" ? "等待确认重置" : window.resetState === "unknown" ? "重置依据未知" : subscriptionFreshness[window.freshness]}</span></div>
        <dl className="subscription-amounts"><div><dt>官方已用</dt><dd>{subscriptionNumber(window.usedPercent, "%")}</dd></div><div><dt>官方剩余</dt><dd>{subscriptionNumber(window.remainingPercent, "%")}</dd></div></dl>
        <p className="subscription-note">下次恢复依据：{subscriptionTime(window.resetsAt)} · 上海时区</p>
        {window.risk && <div className="subscription-risk" data-state={window.risk.state}><strong>{window.risk.label}</strong>{window.risk.exhaustsAtLower && window.risk.exhaustsAtUpper && <p>耗尽情景：{subscriptionTime(window.risk.exhaustsAtLower)}～{subscriptionTime(window.risk.exhaustsAtUpper)}</p>}<p className="subscription-note">{window.risk.notifyEligible ? "Core 已确认本次风险具备提醒资格" : "当前不具备新风险提醒资格"}</p><SubscriptionReasons values={window.risk.reasons} /></div>}
        <SubscriptionReasons values={window.reasons} />
      </article>)}</div></div>)}
    </section>
    <section className="subscription-section" aria-labelledby="subscription-local-title"><div className="subscription-section-heading"><h3 id="subscription-local-title">本机模型用量</h3><span className="subscription-tag">独立账本事实</span></div><p className="subscription-note">Token 为规范化输入加输出，缓存与推理包含项不重复相加；它不是官方剩余额度。</p>
      {!overview.modelUsage.length ? <p className="subscription-empty">当前范围还没有可信模型用量。未知或缺失不按零消耗展示。</p> : <div className="subscription-table-wrap"><table className="subscription-table"><thead><tr><th>模型与口径</th><th>已记录 Token</th><th>确切记录范围（上海时区）</th><th>覆盖与原因</th></tr></thead><tbody>{overview.modelUsage.map((usage, index) => <tr key={`${usage.modelDimension}:${usage.model ?? "unknown"}:${index}`}><td>{subscriptionModelName(usage.model)}<small>{usage.modelDimension === "response" ? "响应模型" : "请求模型"}</small></td><td>{subscriptionNumber(usage.tokens)}</td><td>{subscriptionTime(usage.from)}～{subscriptionTime(usage.to)}</td><td>{subscriptionCoverage(usage.coverage)}<SubscriptionReasons values={usage.reasons} /></td></tr>)}</tbody></table></div>}
    </section>
    <section className="subscription-section" aria-labelledby="subscription-plans-title"><div className="subscription-section-heading"><h3 id="subscription-plans-title">我的使用计划</h3><span className="subscription-tag">用户目标</span></div>{!overview.plans.length ? <p className="subscription-empty">还没有单次计划。可以从现在设定本机 Token 目标，无需等待容量学习。</p> : <div className="subscription-plan-grid">{overview.plans.map((plan) => <PlanCard key={plan.id} plan={plan} window={overview.windows.find((window) => window.id === plan.windowId)} disabled={disabled} onEdit={onEditPlan} onReminders={onToggleReminders} onEnd={onEndPlan} />)}</div>}</section>
    <section className="subscription-section" aria-labelledby="subscription-capacity-title"><div className="subscription-section-heading"><h3 id="subscription-capacity-title">条件容量与学习</h3><span className="subscription-tag">历史等效估算</span></div><p className="subscription-note">仅在标明的账号、规则与使用结构下解释。共享池的各模型情景互斥，不能相加。</p>
      {overview.learningStatus?.status === "unavailable" && <div className="subscription-empty"><strong>当前自动校准暂不可用</strong><SubscriptionReasons values={overview.learningStatus.reasonCodes} /></div>}
      {overview.learningStatus?.status === "insufficient" && <div className="subscription-empty"><strong>自动校准仍在积累有效依据</strong><SubscriptionReasons values={overview.learningStatus.reasonCodes} /></div>}
      <EvidenceAvailability overview={overview} />
      {!overview.calibrations.length ? <p className="subscription-empty">学习中：暂未取得可用容量依据。历史 Token 总量不能直接当作会员容量。</p> : <div className="subscription-window-grid">{overview.calibrations.map((calibration) => calibration.model || calibration.archived ? <details className="subscription-scenario" key={calibration.id}><summary>{calibration.archived ? `历史校准记录 · ${subscriptionTime(calibration.recordedAt)}（不代表当前余额）` : `假设全部用于 ${calibration.model}（与其他模型情景互斥）`}</summary><CapacityCard calibration={calibration} /></details> : <CapacityCard key={calibration.id} calibration={calibration} />)}</div>}
      {!!overview.assumptions.length && <div className="subscription-manual-basis"><h4>手动容量依据</h4><p className="subscription-note">以下为用户假设，与官方事实、自动校准和计划目标分别保存。</p>{overview.assumptions.map((assumption) => <article className="subscription-card" key={assumption.id}><strong>{subscriptionNumber(assumption.capacityTokens, " Token")} · 手动完整窗口假设</strong><p className="subscription-note">{assumption.model ?? "当前模型组合"} · 有效至 {subscriptionTime(assumption.validUntil)}</p><p>{assumption.note}</p><ManualScenario assumption={assumption} />{onEditAssumption && <div className="subscription-inline-actions"><button type="button" disabled={disabled || !overview.capabilities.capacityAssumptions} onClick={() => onEditAssumption(assumption)}>修改手动依据</button></div>}</article>)}</div>}
    </section>
  </>;
}
