import { useState } from "react";
import type { QuotaAssumption, SaveQuotaAssumptionInput, SubscriptionOverview } from "../../shared/subscriptionContracts";
import { buildAssumptionInput, subscriptionDateInput } from "../../shared/subscriptionForms";

export function CapacityAssumptionEditor({ overview, existing, disabled, onSave, onCancel }: {
  overview: SubscriptionOverview;
  existing?: QuotaAssumption | null;
  disabled: boolean;
  onSave: (input: SaveQuotaAssumptionInput) => Promise<void>;
  onCancel: () => void;
}) {
  const [form, setForm] = useState({ windowId: existing?.windowId ?? "", model: existing?.model ?? "", capacityTokens: existing ? String(existing.capacityTokens) : "", validUntil: subscriptionDateInput(existing?.validUntil ?? new Date(Date.now() + 86400000).toISOString()), note: existing?.note ?? "" });
  const [error, setError] = useState<string | null>(null);
  const models = [...new Set(overview.modelUsage.flatMap((usage) => usage.model && usage.model !== "unknown" ? [usage.model] : []))];
  function update(key: keyof typeof form, value: string) { setForm((previous) => ({ ...previous, [key]: value })); setError(null); }
  async function save() {
    if (disabled) return;
    setError(null);
    try { await onSave(buildAssumptionInput(form, overview, Date.now(), existing)); }
    catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)); }
  }
  return <section className="subscription-editor" aria-labelledby="subscription-assumption-editor-title">
    <div className="subscription-section-heading"><h3 id="subscription-assumption-editor-title">{existing ? "修改手动容量依据" : "添加手动容量依据"}</h3><button type="button" onClick={onCancel} disabled={disabled}>取消</button></div>
    <p className="subscription-note">这是独立的手动情景。它不覆盖官方额度，不参加自动校准，也不会改写已有计划。</p>
    <form onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <div className="subscription-form-grid">
        <label>适用窗口<select required value={form.windowId} onChange={(event) => update("windowId", event.target.value)} disabled={disabled}><option value="">请选择窗口</option>{overview.windows.map((window) => <option key={window.id} value={window.id}>{window.bucketName} · {window.label}</option>)}</select></label>
        <label>使用结构<select value={form.model} onChange={(event) => update("model", event.target.value)} disabled={disabled}><option value="">当前模型组合</option>{models.map((model) => <option key={model} value={model}>{model}</option>)}</select></label>
        <label>假设完整窗口容量（Token）<input type="number" min="1" step="1" required value={form.capacityTokens} onChange={(event) => update("capacityTokens", event.target.value)} disabled={disabled} /></label>
        <label>假设有效至（上海时区）<input type="datetime-local" required value={form.validUntil} onChange={(event) => update("validUntil", event.target.value)} disabled={disabled} /></label>
        <label className="subscription-full-field">依据与适用前提<textarea required maxLength={500} rows={3} placeholder="说明信息来源、会员、模型组合和推理/缓存等使用结构" value={form.note} onChange={(event) => update("note", event.target.value)} disabled={disabled} /></label>
      </div>
      {error && <p role="alert" className="subscription-error">{error}</p>}
      <div className="subscription-editor-actions"><button className="subscription-primary" type="submit" disabled={disabled}>{disabled ? "处理中…" : "保存手动依据"}</button></div>
    </form>
  </section>;
}
