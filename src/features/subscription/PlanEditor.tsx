import { useState } from "react";
import type { SaveUsagePlanInput, SubscriptionOverview, UsagePlan } from "../../shared/subscriptionContracts";
import { buildPlanInput, newPlanForm, planFormFromInput, type PlanForm } from "../../shared/subscriptionForms";
import { subscriptionNumber, subscriptionTime } from "../../shared/subscriptionPresentation";

export function PlanEditor({ overview, existing, draft, disabled, onSave, onCancel }: {
  overview: SubscriptionOverview;
  existing?: UsagePlan | null;
  draft?: SaveUsagePlanInput | null;
  disabled: boolean;
  onSave: (input: SaveUsagePlanInput) => Promise<void>;
  onCancel: () => void;
}) {
  const [form, setForm] = useState<PlanForm>(() => existing
    ? planFormFromInput({ ...existing, expectedRevision: existing.revision })
    : draft ? planFormFromInput(draft) : newPlanForm());
  const [error, setError] = useState<string | null>(null);
  const windows = overview.windows.filter((window) => window.canBindPlan && window.resetState === "current");
  const selectedWindow = overview.windows.find((window) => window.id === form.windowId);
  const models = [...new Set(overview.modelUsage.filter((usage) => usage.modelDimension === form.modelDimension && usage.model && usage.model !== "unknown").map((usage) => usage.model!))];
  function update<K extends keyof PlanForm>(key: K, value: PlanForm[K]) {
    setForm((previous) => ({ ...previous, [key]: value }));
    setError(null);
  }
  async function save() {
    if (disabled) return;
    setError(null);
    try { await onSave(buildPlanInput(form, overview, existing)); }
    catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)); }
  }
  return <section className="subscription-editor" aria-labelledby="subscription-plan-editor-title">
    <div className="subscription-section-heading">
      <h3 id="subscription-plan-editor-title">{existing ? "修改计划目标" : draft ? "编辑计划草案" : "新建单次计划"}</h3>
      <button type="button" onClick={onCancel} disabled={disabled}>取消</button>
    </div>
    <p className="subscription-note">来源：{overview.source?.label ?? "未知"}。保存后从生效时刻计量，计划目标不代表会员容量。</p>
    <form onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <div className="subscription-form-grid">
        <label>计划名称<input value={form.title} maxLength={80} placeholder="例如：今天的写作计划" onChange={(event) => update("title", event.target.value)} disabled={disabled} /></label>
        <label>计量类型<select value={form.type} disabled={disabled || !!existing} onChange={(event) => setForm((previous) => ({ ...previous, type: event.target.value as PlanForm["type"], target: "", endPolicy: event.target.value === "percent" ? "window" : "time", windowId: "" }))}>
          <option value="tokens">本机 Token 预算</option>
          <option value="percent" disabled={!windows.length}>官方窗口已用上限</option>
        </select></label>
        {form.type === "tokens" && <>
          <label>模型口径<select value={form.modelDimension} disabled={disabled || !!existing} onChange={(event) => setForm((previous) => ({ ...previous, modelDimension: event.target.value as PlanForm["modelDimension"], model: "" }))}>
            <option value="response">来源记录的响应模型</option><option value="request">来源记录的请求模型</option>
          </select></label>
          <label>统计模型<select value={form.model} disabled={disabled || !!existing} onChange={(event) => update("model", event.target.value)}>
            <option value="">此来源的全部模型</option>{models.map((model) => <option value={model} key={model}>{model}</option>)}
            {existing?.model && !models.includes(existing.model) && <option value={existing.model}>{existing.model}</option>}
          </select></label>
        </>}
        <label>{form.type === "tokens" ? "计划新增 Token" : "窗口已用上限（%）"}<input type="number" min={form.type === "tokens" ? "1" : "0.01"} max={form.type === "percent" ? "100" : undefined} step={form.type === "tokens" ? "1" : "any"} required value={form.target} onChange={(event) => update("target", event.target.value)} disabled={disabled} /></label>
        {form.type === "tokens" && <label>结束依据<select value={form.endPolicy} disabled={disabled || !!existing} onChange={(event) => update("endPolicy", event.target.value as PlanForm["endPolicy"])}>
          <option value="time">指定结束时间</option><option value="window" disabled={!windows.length}>当前官方窗口确认结束</option>
        </select></label>}
        {(form.type === "percent" || form.endPolicy === "window") ? <label>绑定官方窗口<select required value={form.windowId} disabled={disabled || !!existing} onChange={(event) => update("windowId", event.target.value)}>
          <option value="">请选择窗口</option>{windows.map((window) => <option value={window.id} key={window.id}>{window.bucketName} · {window.label}</option>)}
          {existing?.windowId && !windows.some((window) => window.id === existing.windowId) && <option value={existing.windowId}>原绑定窗口（当前待确认）</option>}
        </select></label> : <label>结束时间（上海时区）<input type="datetime-local" required value={form.endsAt} disabled={disabled || !!existing} onChange={(event) => update("endsAt", event.target.value)} /></label>}
      </div>
      <label className="subscription-checkbox"><input type="checkbox" checked={form.remindersEnabled} onChange={(event) => update("remindersEnabled", event.target.checked)} disabled={disabled} />启用面板和收件箱提醒；系统通知与桌宠提示以当前资格为准</label>
      {selectedWindow && <p className="subscription-note">当前官方已用 {subscriptionNumber(selectedWindow.usedPercent, "%")} · 官方剩余 {subscriptionNumber(selectedWindow.remainingPercent, "%")} · 恢复依据 {subscriptionTime(selectedWindow.resetsAt)}</p>}
      {existing && <p className="subscription-note">当前计划已用 {subscriptionNumber(existing.used, existing.type === "tokens" ? " Token" : "%")} · 计划剩余 {subscriptionNumber(existing.remaining, existing.type === "tokens" ? " Token" : " 个百分点")}</p>}
      <p className="subscription-note">{existing ? `保留原起点 ${subscriptionTime(existing.effectiveFrom)} 与已累计用量；目标修改不会清零，也不补发旧越线。` : "起点为服务端保存成功时刻。本机用量、计划余额与官方额度分别计量。"}</p>
      {form.type === "percent" && <p className="subscription-note">填写窗口累计已用上限，例如当前 60%、目标 80%，表示还可增加约 20 个百分点。</p>}
      {error && <p role="alert" className="subscription-error">{error}</p>}
      <div className="subscription-editor-actions"><button className="subscription-primary" type="submit" disabled={disabled}>{disabled ? "处理中…" : existing ? "保存修订" : "保存并启用计划"}</button></div>
    </form>
  </section>;
}
