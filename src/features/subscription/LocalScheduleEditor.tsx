import { useState } from "react";
import type { LocalScheduleTemplate, LocalSchedulesOverview, SaveLocalSchedule } from "../../shared/localScheduleContracts";
import { buildLocalScheduleInput, localScheduleFormFromTemplate, newLocalScheduleForm, type LocalScheduleForm } from "../../shared/localScheduleForms";

export function LocalScheduleEditor({ overview, existing, disabled, onSave, onCancel }: {
  overview: LocalSchedulesOverview; existing: LocalScheduleTemplate | null; disabled: boolean;
  onSave: (value: SaveLocalSchedule) => Promise<boolean>; onCancel: () => void;
}) {
  const [form, setForm] = useState(() => existing ? localScheduleFormFromTemplate(existing) : newLocalScheduleForm());
  const [error, setError] = useState<string | null>(null);
  const [unconfirmed, setUnconfirmed] = useState(false), [checked, setChecked] = useState(false);
  const update = <K extends keyof LocalScheduleForm>(key: K, value: LocalScheduleForm[K]) => { setForm(f => ({ ...f, [key]: value })); setError(null); };
  async function save() {
    if (disabled || (unconfirmed && !checked)) return;
    setError(null);
    try {
      if (!await onSave(buildLocalScheduleInput(form, overview, existing))) {
        setError("保存尚未确认。请先更新视图核对结果，再决定是否提交。");
        if (!existing) { setUnconfirmed(true); setChecked(false); }
      }
    } catch (failure) { setError(failure instanceof Error ? failure.message : "计划校验失败。"); }
  }
  return <section className="local-schedule-editor" aria-labelledby="local-schedule-editor-title">
    <div className="local-schedule-heading"><h3 id="local-schedule-editor-title">{existing ? "编辑后续周期模板" : "新建周期模板"}</h3><button type="button" onClick={onCancel} disabled={disabled}>取消编辑</button></div>
    <p className="local-schedule-note">{existing ? "修改仅用于下一实例，当前实例的目标、单位、计量起点和已记录量保持原快照。" : "首期从保存成功与周期起点中较晚的时刻计量。选择下一周期可等待完整的新周期。"}</p>
    <form onSubmit={event => { event.preventDefault(); void save(); }}>
      <div className="local-schedule-grid">
        <label>模板名称<input required maxLength={80} value={form.title} disabled={disabled} onChange={event => update("title", event.target.value)} /></label>
        <label>计量单位<select value={form.unit} disabled={disabled} onChange={event => setForm(f => ({ ...f, unit: event.target.value as LocalScheduleForm["unit"], target: "", model: "", remindersEnabled: event.target.value === "tokens" && f.remindersEnabled }))}><option value="tokens">Tokens · 真实本机账本</option><option value="requests">requests · 仅手动记录</option><option value="credits">credits · 仅手动记录</option></select></label>
        <label>每期目标<input required inputMode={form.unit === "credits" ? "decimal" : "numeric"} maxLength={32} value={form.target} disabled={disabled} placeholder={form.unit === "credits" ? "例如 12.500000" : "正整数"} onChange={event => update("target", event.target.value)} /></label>
        <label>周期<select value={form.kind} disabled={disabled} onChange={event => setForm(f => ({ ...f, kind: event.target.value as LocalScheduleForm["kind"], ...(event.target.value === "once" ? { startPolicy: "now", autoRenew: false } : {}) }))}><option value="once">单次</option><option value="daily">每天</option><option value="weekly">每周</option><option value="monthly">每月</option></select></label>
        <label>周期起点（上海时间）<input type="datetime-local" step="0.001" required value={form.anchorAt} disabled={disabled} onChange={event => update("anchorAt", event.target.value)} /></label>
        {form.kind === "once" ? <label>单次结束（上海时间）<input type="datetime-local" step="0.001" required value={form.endsAt} disabled={disabled} onChange={event => update("endsAt", event.target.value)} /></label> : <label>首次开始<select value={form.startPolicy} disabled={disabled} onChange={event => update("startPolicy", event.target.value as LocalScheduleForm["startPolicy"])}><option value="now">本期开始（可能只有首期部分区间）</option><option value="next_period">下一完整周期</option></select></label>}
        {form.unit === "tokens" && <><label>模型口径<select value={form.modelDimension} disabled={disabled} onChange={event => update("modelDimension", event.target.value as LocalScheduleForm["modelDimension"])}><option value="response">实际响应模型</option><option value="request">请求模型</option></select></label><label>模型名称（精确匹配，可留空）<input maxLength={512} value={form.model} disabled={disabled} placeholder="留空：此来源全部模型" onChange={event => update("model", event.target.value)} /></label></>}
      </div>
      {form.kind !== "once" && <label className="local-schedule-checkbox"><input type="checkbox" checked={form.autoRenew} disabled={disabled} onChange={event => update("autoRenew", event.target.checked)} />到期自动续建下一实例（默认关闭，须明确勾选）</label>}
      {form.kind === "monthly" && <p className="local-schedule-note">每月按原起点日期计算；当月没有该日时使用月末，下个月仍回到原日期。</p>}
      {form.unit !== "tokens" && <p className="local-schedule-note">这里只累计你手动填写的记录，全部实际消耗保持未知。requests 不代表从响应用量推算的 HTTP 请求数，credits 不换算为 Token。</p>}
      <fieldset className="local-schedule-allocation" disabled={disabled}>
        <legend>自由日程分段</legend><p className="local-schedule-note">留空表示整期均匀安排。天数与时间均从周期起点累计，例如起点后 0 天 09:00 表示经过 9 小时；分段不能重叠，权重须合计 100%。每月分段最多前 28 天。</p>
        {form.allocation.map((part, index) => <div className="local-schedule-part" key={index}>
          <label>分段 {index + 1} 开始天数<input type="number" min="0" step="1" value={part.startDay} onChange={event => update("allocation", form.allocation.map((v, i) => i === index ? { ...v, startDay: event.target.value } : v))} /></label>
          <label>开始时间<input type="time" value={part.startTime} onChange={event => update("allocation", form.allocation.map((v, i) => i === index ? { ...v, startTime: event.target.value } : v))} /></label>
          <label>结束天数<input type="number" min="0" step="1" value={part.endDay} onChange={event => update("allocation", form.allocation.map((v, i) => i === index ? { ...v, endDay: event.target.value } : v))} /></label>
          <label>结束时间<input type="time" value={part.endTime} onChange={event => update("allocation", form.allocation.map((v, i) => i === index ? { ...v, endTime: event.target.value } : v))} /></label>
          <label>权重（%）<input inputMode="decimal" maxLength={6} value={part.weightPercent} onChange={event => update("allocation", form.allocation.map((v, i) => i === index ? { ...v, weightPercent: event.target.value } : v))} /></label>
          <button type="button" aria-label={`删除分段 ${index + 1}`} onClick={() => update("allocation", form.allocation.filter((_, i) => i !== index))}>删除</button>
        </div>)}
        <button type="button" disabled={disabled || form.allocation.length >= 64} onClick={() => update("allocation", [...form.allocation, { startDay: "0", startTime: "00:00", endDay: form.kind === "weekly" ? "7" : form.kind === "monthly" ? "28" : "1", endTime: "00:00", weightPercent: form.allocation.length ? "" : "100" }])}>添加分段</button>
      </fieldset>
      <label className="local-schedule-checkbox"><input type="checkbox" checked={form.unit === "tokens" && form.remindersEnabled} disabled={disabled || form.unit !== "tokens"} onChange={event => update("remindersEnabled", event.target.checked)} />仅 Token 实例可复用现有面板提醒；系统与桌宠展示仍受当前资格限制</label>
      {form.unit !== "tokens" && <p className="local-schedule-note">requests / credits 提醒尚不支持；手动记录不会触发额度或完成提醒。</p>}
      {error && <p className="local-schedule-error" role="alert">{error}</p>}
      {unconfirmed && <label className="local-schedule-checkbox"><input type="checkbox" checked={checked} disabled={disabled} onChange={event => setChecked(event.target.checked)} />我已核对更新后的模板，仍需新建这一份</label>}
      <div className="local-schedule-actions"><button className="local-schedule-primary" type="submit" disabled={disabled || (unconfirmed && !checked)}>{disabled ? "处理中…" : existing ? "保存下一实例模板" : "保存模板"}</button></div>
    </form>
  </section>;
}
