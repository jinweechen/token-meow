import { useEffect, useRef, useState } from "react";
import type { SourceInstance } from "../../shared/contracts";
import { LocalScheduleReadModel, localScheduleClient, type LocalScheduleViewState } from "../../shared/localScheduleClient";
import type { LocalConsumptionEntry, LocalScheduleState, LocalScheduleTemplate, LocalSchedulesOverview, RecordLocalConsumption, SaveLocalSchedule } from "../../shared/localScheduleContracts";
import { sourceKindName } from "../../shared/sourceKind";
import { subscriptionReason, subscriptionTime } from "../../shared/subscriptionPresentation";
import { LocalScheduleEditor } from "./LocalScheduleEditor";
import { LocalScheduleView } from "./LocalScheduleView";
import "./localSchedule.css";

const initial = (): LocalScheduleViewState => ({ overview: null, pending: false, fresh: false, action: null, error: null, notice: null, formEpoch: 0 });
export interface LocalSchedulePanelProps { sources: SourceInstance[]; scopeKey: string; enabled: boolean; privacyMode: boolean }

export function LocalSchedulePanel({ sources, scopeKey, enabled, privacyMode }: LocalSchedulePanelProps) {
  const [selectedId, setSelectedId] = useState(() => sources.find(s => s.primary && s.enabled)?.id ?? sources.find(s => s.enabled)?.id ?? "");
  const source = sources.find(s => s.enabled && s.id === selectedId) ?? sources.find(s => s.enabled);
  const sourceId = source?.id ?? "";
  const domain = JSON.stringify([scopeKey, enabled, privacyMode, sourceId, source?.clientKind, source?.root, source?.enabled, source?.environment, source?.configurationRevision]);
  const domainNow = useRef(domain); domainNow.current = domain;
  const controller = useRef<{ domain: string; model: LocalScheduleReadModel } | null>(null);
  const [view, setView] = useState<{ domain: string; value: LocalScheduleViewState }>({ domain, value: initial() });
  const [editor, setEditor] = useState<{ template: LocalScheduleTemplate | null; overview: LocalSchedulesOverview; key: number } | null>(null);
  const editorKey = useRef(0);
  useEffect(() => {
    setEditor(null); setView({ domain, value: initial() });
    if (!enabled || privacyMode || !sourceId) return;
    const model = new LocalScheduleReadModel(localScheduleClient, sourceId, value => {
      if (domainNow.current === domain) setView({ domain, value });
    });
    controller.current = { domain, model }; model.start();
    // 只在可见时更新本地投影，不请求官方账户或模型。
    const visibility = () => { if (!document.hidden) model.reload(); };
    const timer = window.setInterval(() => { if (!document.hidden) model.reload(); }, 15000);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      model.dispose(); window.clearInterval(timer); document.removeEventListener("visibilitychange", visibility);
      if (controller.current?.model === model) controller.current = null;
    };
  }, [domain, enabled, privacyMode, sourceId]);
  const state = view.domain === domain && enabled && !privacyMode ? view.value : initial();
  const overview = state.overview;
  useEffect(() => { setEditor(null); }, [state.formEpoch]);
  const disabled = !enabled || privacyMode || !overview?.source.enabled || !state.fresh || state.pending || view.domain !== domain;
  const model = () => controller.current?.domain === domainNow.current && enabled && !privacyMode ? controller.current.model : null;
  async function save(input: SaveLocalSchedule) {
    const owner = model(); if (!owner) return false;
    const result = await owner.mutate("save", input.expectedScope, current => localScheduleClient.save(input, current), input.id ? "后续模板已保存；当前实例保持原快照。" : "周期模板已保存。");
    if (result && domainNow.current === domain) setEditor(null);
    return result;
  }
  function status(template: LocalScheduleTemplate, next: LocalScheduleState) {
    const owner = model(); if (!owner || !overview) return Promise.resolve(false);
    const payload = { sourceId, expectedScope: overview.scopeToken, id: template.id, expectedRevision: template.revision, state: next };
    return owner.mutate("status", overview.scopeToken, current => localScheduleClient.status(payload, current), next === "disabled" ? "模板已停用，当前实例已结束。" : next === "paused" ? "已暂停续建，本期继续计量。" : "已恢复后续续建。");
  }
  function record(input: RecordLocalConsumption) {
    const owner = model(); if (!owner) return Promise.resolve(false);
    return owner.mutate("record", input.expectedScope, current => localScheduleClient.record(input, current), "手动记录已保存；全部实际消耗仍保持未知。");
  }
  function voidEntry(entry: LocalConsumptionEntry, idempotencyKey: string) {
    const owner = model(); if (!owner || !overview) return Promise.resolve(false);
    const payload = { sourceId, expectedScope: overview.scopeToken, entryId: entry.id, expectedRevision: entry.revision, idempotencyKey };
    return owner.mutate("void", overview.scopeToken, current => localScheduleClient.void(payload, current), "记录已撤销，原始记录保留。");
  }
  const edit = (template: LocalScheduleTemplate | null) => { if (overview && !disabled) setEditor({ template, overview, key: ++editorKey.current }); };
  return <section className="local-schedule-panel" aria-labelledby="local-schedule-title">
    <header className="local-schedule-heading"><div><h2 id="local-schedule-title">本地周期与自由日程</h2><p className="local-schedule-note">为指定来源安排本地目标。目录不是账号；切换这里的来源不会改变官方主来源，目标也不代表会员额度。</p></div><div className="local-schedule-actions"><button type="button" disabled={!enabled || privacyMode || !sourceId || state.pending} onClick={() => model()?.reload()}>更新本地计划</button><button className="local-schedule-primary" type="button" disabled={disabled} onClick={() => edit(null)}>新建周期模板</button></div></header>
    <label className="local-schedule-source">计划来源<select value={sourceId} disabled={!enabled || privacyMode || !sources.some(s => s.enabled)} onChange={event => setSelectedId(event.target.value)}>{!sourceId && <option value="">暂无已启用来源</option>}{sources.map((s, index) => <option key={s.id} value={s.id} disabled={!s.enabled}>{sourceKindName(s.clientKind)} · 来源 {index + 1}{s.primary ? "（官方主来源）" : ""}{s.enabled ? "" : "（已停用）"}{privacyMode ? "" : ` · ${s.root}`}</option>)}</select></label>
    <p className="local-schedule-note">固定上海时区 +08:00 · Tokens 来自真实账本；requests / credits 仅手动记录，实际总量未知。</p>
    {!enabled && <p className="local-schedule-empty">本地计划当前暂停读取，待来源操作完成后重新载入。</p>}
    {enabled && privacyMode && <p className="local-schedule-empty">隐私模式已隐藏模板和手动记录，并关闭计划编辑。</p>}
    {enabled && !privacyMode && !sourceId && <p className="local-schedule-empty">请先在采集健康中登记并启用一个来源。</p>}
    {state.error && <p className="local-schedule-error" role="alert">{state.error} 请核对更新后的视图；保存失败不会自动重发写入。</p>}
    {state.notice && <p className="local-schedule-message" role="status">{state.notice}</p>}
    {state.pending && !overview && <p className="local-schedule-empty" role="status">正在读取本地计划…</p>}
    {overview && <><p className="local-schedule-note">视图基准 {subscriptionTime(overview.asOf)} · 修订 {overview.revision}{!state.fresh ? " · 正在核对当前范围，写入暂不可用" : ""}</p>
      {!!overview.reasons.length && <ul className="local-schedule-reasons">{overview.reasons.map(reason => <li key={reason}>{subscriptionReason(reason)}</li>)}</ul>}
      {editor && <LocalScheduleEditor key={editor.key} overview={editor.overview} existing={editor.template} disabled={disabled} onSave={save} onCancel={() => setEditor(null)} />}
      <LocalScheduleView key={state.formEpoch} overview={overview} disabled={disabled} onEdit={edit} onStatus={status} onRecord={record} onVoid={voidEntry} />
    </>}
  </section>;
}
