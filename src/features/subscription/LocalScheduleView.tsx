import { useEffect, useRef, useState } from "react";
import type { LocalConsumptionEntry, LocalScheduleInstance, LocalScheduleState, LocalScheduleTemplate, LocalSchedulesOverview, RecordLocalConsumption } from "../../shared/localScheduleContracts";
import { buildLocalConsumptionInput, localScheduleDateInput } from "../../shared/localScheduleForms";
import { subscriptionReason, subscriptionTime } from "../../shared/subscriptionPresentation";

const unitName = (unit: string) => unit === "tokens" ? "Tokens" : unit;
const kindName = (kind: string) => ({ once: "单次", daily: "每天", weekly: "每周", monthly: "每月" })[kind] ?? "未知周期";
const exactAmount = (value: string | null, unit: string) => {
  if (value === null) return "未知";
  const [whole, fraction] = value.split(".");
  return `${whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",")}${fraction === undefined ? "" : `.${fraction}`} ${unitName(unit)}`;
};
const partOffset = (value: number) => `起点后 ${Math.floor(value / 1440)} 天 ${String(Math.floor(value % 1440 / 60)).padStart(2, "0")}:${String(value % 60).padStart(2, "0")}`;

export function LocalScheduleView({ overview, disabled, onEdit, onStatus, onRecord, onVoid }: {
  overview: LocalSchedulesOverview; disabled: boolean; onEdit: (template: LocalScheduleTemplate) => void;
  onStatus: (template: LocalScheduleTemplate, state: LocalScheduleState) => Promise<boolean>;
  onRecord: (input: RecordLocalConsumption) => Promise<boolean>;
  onVoid: (entry: LocalConsumptionEntry, idempotencyKey: string) => Promise<boolean>;
}) {
  const [record, setRecord] = useState<{ instanceId: string; scopeToken: string; idempotencyKey: string; amount: string; occurredAt: string; note: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const voidKeys = useRef(new Map<string, string>());
  useEffect(() => {
    if (record && (record.scopeToken !== overview.scopeToken || overview.instances.some(i => i.entries.some(e => e.id === record.idempotencyKey)))) {
      setRecord(null); setError(null);
    }
  }, [overview, record]);
  function openRecord(instance: LocalScheduleInstance) {
    setError(null); setRecord({ instanceId: instance.id, scopeToken: overview.scopeToken, idempotencyKey: crypto.randomUUID(), amount: "",
      occurredAt: localScheduleDateInput(new Date().toISOString(), "milliseconds"), note: "" });
  }
  function updateRecord(key: "amount" | "occurredAt" | "note", value: string) {
    setError(null); setRecord(previous => previous ? { ...previous, [key]: value, idempotencyKey: crypto.randomUUID() } : previous);
  }
  async function saveRecord(instance: LocalScheduleInstance) {
    if (!record || disabled) return;
    try {
      const input = buildLocalConsumptionInput(record, { ...overview, scopeToken: record.scopeToken }, instance);
      if (await onRecord(input)) { setRecord(null); setError(null); }
      else setError("记录尚未确认，请核对更新后的记录；原记录身份保留，原样重试不会重复累计。");
    } catch (failure) { setError(failure instanceof Error ? failure.message : "消耗记录校验失败。"); }
  }
  async function voidEntry(entry: LocalConsumptionEntry) {
    const key = `${entry.id}:${entry.revision}`;
    if (!voidKeys.current.has(key)) voidKeys.current.set(key, crypto.randomUUID());
    await onVoid(entry, voidKeys.current.get(key)!);
  }
  const instanceCard = (instance: LocalScheduleInstance) => {
    const manual = instance.unit !== "tokens";
    const end = instance.endedAt && Date.parse(instance.endedAt) < Date.parse(instance.periodEndsAt) ? instance.endedAt : instance.periodEndsAt;
    return <article className="local-schedule-instance" key={instance.id} aria-label={`${instance.title} 本期实例`}>
      <div className="local-schedule-heading"><h4>{instance.title}</h4><span className="local-schedule-tag">{instance.lifecycle === "active" ? "本期计量中" : "本期已结束"}</span></div>
      <p className="local-schedule-note">计量区间 {subscriptionTime(instance.effectiveFrom)} ～ {subscriptionTime(end)}（不含结束时刻）</p>
      {Date.parse(instance.effectiveFrom) > Date.parse(instance.periodStartsAt) && <p className="local-schedule-note">首期或生效期为部分区间，目标和曲线沿用本实例快照。</p>}
      <dl className="local-schedule-values"><div><dt>本期目标</dt><dd>{exactAmount(instance.target, instance.unit)}</dd></div>
        <div><dt>{manual ? "手动已记录" : "可信账本已用"}</dt><dd>{exactAmount(manual ? instance.recordedUsed : instance.used, instance.unit)}</dd></div>
        <div><dt>{manual ? "目标减已记录" : "计划余额"}</dt><dd>{exactAmount(instance.remaining, instance.unit)}</dd></div>
        <div><dt>此时计划量</dt><dd>{exactAmount(instance.plannedByNow, instance.unit)}</dd></div></dl>
      <p className="local-schedule-note">{manual ? "全部实际消耗：未知 · 手动记录仅覆盖你填入的部分。" : "仅当前来源真实 Token 计划投影；缺失或异常计量保持未知。"} {instance.progressPercent === null ? "进度未知" : `已记录进度 ${new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 2 }).format(instance.progressPercent)}%`}</p>
      {instance.evaluationState !== "ready" && <p className="local-schedule-note">{instance.evaluationState === "pending" ? "等待计量依据" : "当前完整计量状态未知"}</p>}
      {instance.reasons.length > 0 && <ul className="local-schedule-reasons">{instance.reasons.map(reason => <li key={reason}>{subscriptionReason(reason)}</li>)}</ul>}
      {manual && <>
        <div className="local-schedule-actions"><button type="button" disabled={disabled} onClick={() => openRecord(instance)}>记录 {instance.unit} 消耗</button></div>
        {record?.instanceId === instance.id && <form className="local-schedule-record" aria-label={`记录 ${instance.title} 消耗`} onSubmit={event => { event.preventDefault(); void saveRecord(instance); }}>
          <div className="local-schedule-grid"><label>本次手动记录量<input required maxLength={32} inputMode={instance.unit === "credits" ? "decimal" : "numeric"} value={record.amount} disabled={disabled} onChange={event => updateRecord("amount", event.target.value)} /></label>
            <label>发生时间（上海时间）<input type="datetime-local" step="0.001" required value={record.occurredAt} disabled={disabled} onChange={event => updateRecord("occurredAt", event.target.value)} /></label>
            <label className="local-schedule-wide">记录说明（可留空）<input maxLength={500} value={record.note} disabled={disabled} onChange={event => updateRecord("note", event.target.value)} /></label></div>
          {error && <p className="local-schedule-error" role="alert">{error}</p>}
          <div className="local-schedule-actions"><button type="submit" disabled={disabled}>保存手动记录</button><button type="button" disabled={disabled} onClick={() => { setRecord(null); setError(null); }}>取消记录</button></div>
        </form>}
        {!!instance.entries.length && <details className="local-schedule-history"><summary>手动记录（{instance.entries.length} 笔）</summary><div className="local-schedule-table-wrap"><table><thead><tr><th>发生时间</th><th>记录量</th><th>说明</th><th>状态</th></tr></thead><tbody>{instance.entries.map(entry => <tr key={entry.id}><td>{subscriptionTime(entry.occurredAt)}</td><td>{exactAmount(entry.amount, instance.unit)}</td><td>{entry.note || "未填写"}</td><td>{entry.voided ? "已撤销" : <button type="button" disabled={disabled} onClick={() => { void voidEntry(entry); }}>撤销记录</button>}</td></tr>)}</tbody></table></div><p className="local-schedule-note">撤销保留原始记录与撤销依据，不删除账本。</p></details>}
      </>}
    </article>;
  };
  return <div className="local-schedule-list">
    {!overview.templates.length && <p className="local-schedule-empty">这个来源尚无周期模板。新建后可单次使用，或明确开启自动续建。</p>}
    {overview.templates.map(template => <section className="local-schedule-template" key={template.id} aria-label={`${template.title} 周期模板`}>
      <div className="local-schedule-heading"><div><h3>{template.title}</h3><p className="local-schedule-note">{kindName(template.rule.kind)} · {exactAmount(template.target, template.unit)} · {template.state === "active" ? "模板已启用" : template.state === "paused" ? "已暂停续建" : "已停用"} · 自动续建{template.autoRenew ? "已开启" : "未开启"}</p></div>
        <div className="local-schedule-actions"><button type="button" disabled={disabled || template.state === "disabled" || (template.rule.kind === "once" && Date.parse(template.firstStartsAt) <= Date.now())} onClick={() => onEdit(template)}>编辑后续模板</button>
          {template.state !== "disabled" && template.rule.kind !== "once" && <button type="button" disabled={disabled} onClick={() => { void onStatus(template, template.state === "paused" ? "active" : "paused"); }}>{template.state === "paused" ? "恢复续建" : "暂停续建"}</button>}
          {template.state !== "disabled" && <button type="button" disabled={disabled} onClick={() => { void onStatus(template, "disabled"); }}>停用并结束当前</button>}</div></div>
      <p className="local-schedule-note">起点 {subscriptionTime(template.rule.anchorAt)} · {template.nextPeriodStartsAt ? `下一期 ${subscriptionTime(template.nextPeriodStartsAt)}` : "无待续建周期"} · 模板修订 {template.revision}</p>
      {template.bindingState !== "ready" && <p className="local-schedule-note">{template.bindingState === "pending_rebind" ? "来源配置已改变，等待重新确认绑定；不会沿用旧配置续建。" : "来源当前不可用，续建暂停。"}</p>}
      {template.allocation.length ? <details className="local-schedule-history"><summary>查看自由分段（{template.allocation.length} 段）</summary><ul>{template.allocation.map((part, index) => <li key={index}>{partOffset(part.startMinute)} ～ {partOffset(part.endMinute)} · {part.weightBps / 100}%</li>)}</ul></details> : <p className="local-schedule-note">整期均匀安排</p>}
      <p className="local-schedule-note">暂停只停止续建，本期继续计量；停用会结束当前实例，不能恢复原实例。</p>
      {overview.instances.filter(instance => instance.templateId === template.id).map(instanceCard)}
    </section>)}
  </div>;
}
