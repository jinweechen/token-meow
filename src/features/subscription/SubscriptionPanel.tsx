import { useEffect, useRef, useState } from "react";
import { isDesktop } from "../../shared/bridge";
import { subscriptionClient, type SubscriptionClient } from "../../shared/subscriptionClient";
import type { QuotaAssumption, SaveQuotaAssumptionInput, SaveUsagePlanInput, SubscriptionOverview, UsagePlan, UsagePlanDraft } from "../../shared/subscriptionContracts";
import { subscriptionTime } from "../../shared/subscriptionPresentation";
import { CapacityAssumptionEditor } from "./CapacityAssumptionEditor";
import { PlanEditor } from "./PlanEditor";
import { SubscriptionView, UsagePlanDraftBasis } from "./SubscriptionView";
import "./subscription.css";

export interface SubscriptionPanelProps {
  /** Current primary source identity; changing it discards all pending UI work. */
  sourceId?: string | null;
  /** Invalidate after data clearing, privacy changes or another domain mutation. */
  scopeKey?: string | number;
  /** Parent may refresh local projections after a source/ledger operation. */
  refreshKey?: string | number;
  enabled?: boolean;
  privacyMode?: boolean;
  client?: SubscriptionClient;
}

export function SubscriptionPanel({ sourceId, scopeKey, refreshKey, enabled = isDesktop, privacyMode = false, client = subscriptionClient }: SubscriptionPanelProps) {
  const [overview, setOverview] = useState<SubscriptionOverview | null>(null);
  const [viewDomain, setViewDomain] = useState({ sourceId, scopeKey, enabled, privacyMode, client });
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [action, setAction] = useState<string | null>(null);
  const [editor, setEditor] = useState<{ key: number; plan: UsagePlan | null; draft: SaveUsagePlanInput | null; scopeToken?: string } | null>(null);
  const [basisOpen, setBasisOpen] = useState(false);
  const [editingBasis, setEditingBasis] = useState<QuotaAssumption | null>(null);
  const [basisScope, setBasisScope] = useState<string | undefined>();
  const acceptedScope = useRef<string | undefined>(undefined);
  const [draft, setDraft] = useState<UsagePlanDraft | null>(null);
  const [draftWindow, setDraftWindow] = useState("");
  const [draftModel, setDraftModel] = useState("");
  const [draftDimension, setDraftDimension] = useState<"response" | "request">("response");
  const [reserve, setReserve] = useState("20");
  const generation = useRef(0);
  const request = useRef(0);
  const pendingRead = useRef<{ generation: number; ticket: number } | null>(null);
  const busy = useRef(false);
  const editorKey = useRef(0);
  const refreshView = useRef<() => Promise<void>>(async () => {});
  function acceptOverview(value: SubscriptionOverview) {
    if (acceptedScope.current !== value.scopeToken) {
      setEditor(null); setBasisOpen(false); setEditingBasis(null); setDraft(null);
    }
    acceptedScope.current = value.scopeToken;
    setOverview(value);
  }

  useEffect(() => {
    const current = ++generation.current;
    request.current++;
    pendingRead.current = null;
    busy.current = false;
    acceptedScope.current = undefined;
    setViewDomain({ sourceId, scopeKey, enabled, privacyMode, client });
    setOverview(null); setError(null); setMessage(null); setAction(null); setEditor(null); setBasisOpen(false); setEditingBasis(null); setDraft(null); setDraftWindow(""); setDraftModel("");
    if (!enabled) { setLoading(false); return; }
    async function load() {
      if (busy.current || current !== generation.current || pendingRead.current?.generation === current) return;
      const ticket = ++request.current;
      pendingRead.current = { generation: current, ticket };
      setLoading(true);
      try {
        const value = await client.readOverview();
        if (current !== generation.current || ticket !== request.current) return;
        if (sourceId != null && value.source?.id !== sourceId) throw Error("来源已变化，等待当前来源的订阅视图。");
        acceptOverview(value); setError(null);
      } catch (failure) {
        if (current === generation.current && ticket === request.current) setError(failure instanceof Error ? failure.message : String(failure));
      } finally {
        if (pendingRead.current?.ticket === ticket && pendingRead.current.generation === current) pendingRead.current = null;
        if (current === generation.current && ticket === request.current) setLoading(false);
      }
    }
    refreshView.current = load;
    void load();
    // A local read has no official collection side effects.
    const timer = window.setInterval(() => { void load(); }, 15000);
    return () => { window.clearInterval(timer); if (generation.current === current) generation.current++; };
  }, [client, enabled, sourceId, scopeKey, privacyMode]);

  useEffect(() => { if (enabled && refreshKey !== undefined) void refreshView.current(); }, [enabled, refreshKey]);

  async function mutate(name: string, operation: () => Promise<SubscriptionOverview>, success: string) {
    if (busy.current || !enabled || (privacyMode && name !== "official")) return false;
    const current = generation.current;
    busy.current = true; request.current++; setLoading(false); setAction(name); setError(null); setMessage(null);
    try {
      const value = await operation();
      if (current !== generation.current) return false;
      if (sourceId != null && value.source?.id !== sourceId) throw Error("操作后的来源已变化，请刷新后检查计划状态。");
      acceptOverview(value); setMessage(success);
      return true;
    } catch (failure) {
      if (current === generation.current) {
        setError(failure instanceof Error ? failure.message : String(failure));
        throw failure;
      }
      return false;
    } finally {
      if (current === generation.current) { busy.current = false; setAction(null); }
    }
  }
  async function savePlan(input: SaveUsagePlanInput, scopeToken?: string) {
    if (await mutate("plan", () => client.savePlan(input, scopeToken), input.id ? "计划修订已保存；原起点和累计用量保留。" : "计划已保存并启用。")) { setEditor(null); setDraft(null); }
  }
  async function saveAssumption(input: SaveQuotaAssumptionInput) {
    if (await mutate("assumption", () => client.saveAssumption(input, basisScope), "手动容量依据已保存，现有计划保持独立。")) setBasisOpen(false);
  }
  function edit(plan: UsagePlan | null, nextDraft: SaveUsagePlanInput | null = null) {
    setEditor({ key: ++editorKey.current, plan, draft: nextDraft, scopeToken: overview?.scopeToken }); setBasisOpen(false); setError(null); setMessage(null);
  }
  async function officialRefresh() {
    await mutate("official", async () => { await client.refreshOfficial(); return client.readOverview(); }, "官方读取已完成，当前资格与原因见下方。").catch(() => {});
  }
  async function status(plan: UsagePlan, end = false) {
    await mutate("status", () => client.setPlanStatus({ id: plan.id, expectedRevision: plan.revision, ...(end ? { end: true } : { remindersEnabled: !plan.remindersEnabled }) }, overview?.scopeToken), end ? "计划已结束；重新开始需新建计划。" : plan.remindersEnabled ? "已暂停提醒，仍持续计量和预测。" : "已恢复提醒，历史越线不补播。").catch(() => {});
  }
  async function generate(intent: "remaining" | "habit") {
    if (!overview?.source || busy.current || privacyMode) return;
    const reservePercent = Number(reserve);
    if (!reserve.trim() || !Number.isFinite(reservePercent) || reservePercent < 0 || reservePercent >= 100) { setError("预留须大于等于 0 且小于 100%。"); return; }
    if (intent === "remaining" && !overview.windows.some((window) => window.id === draftWindow && window.canBindPlan)) { setError("按当前余量生成时，请先选择可绑定的官方窗口。"); return; }
    const current = generation.current;
    const scopeToken = overview.scopeToken;
    busy.current = true; request.current++; setLoading(false); setAction("draft"); setError(null); setMessage(null);
    try {
      const value = await client.generateDraft({ intent, sourceId: overview.source.id, windowId: draftWindow || null, model: draftModel || null, modelDimension: draftDimension, reservePercent }, overview.scopeToken);
      if (current !== generation.current || acceptedScope.current !== scopeToken) return;
      setDraft(value);
      if (value.plan) edit(null, value.plan);
    } catch (failure) { if (current === generation.current) setError(failure instanceof Error ? failure.message : String(failure)); }
    finally { if (current === generation.current) { busy.current = false; setAction(null); } }
  }
  const domainMatches = viewDomain.sourceId === sourceId && viewDomain.scopeKey === scopeKey && viewDomain.enabled === enabled && viewDomain.privacyMode === privacyMode && viewDomain.client === client;
  const readDisabled = !!action || !enabled || !domainMatches;
  const disabled = readDisabled || privacyMode;
  // Hide the previous source synchronously, before the effect clears its state.
  const visibleOverview = !domainMatches || !enabled || (sourceId != null && overview?.source?.id !== sourceId) ? null : overview;
  return <section className="subscription-panel" aria-labelledby="subscription-title">
    <header className="subscription-header"><div><h2 id="subscription-title">订阅与计划</h2><p>分别查看官方额度、本机用量、用户目标与条件容量。</p>{visibleOverview && <p className="subscription-note">视图基准 {subscriptionTime(visibleOverview.asOf)} · 上海时区 · 修订 {visibleOverview.revision}</p>}</div><div className="subscription-inline-actions"><button type="button" disabled={readDisabled || loading} onClick={() => { void refreshView.current(); }}>{loading ? "读取中…" : "更新视图"}</button><button type="button" disabled={readDisabled} onClick={() => { void officialRefresh(); }}>{action === "official" ? "读取官方额度…" : "刷新官方额度"}</button><button className="subscription-primary" type="button" disabled={disabled || !visibleOverview?.source || !visibleOverview.capabilities.manualPlans} onClick={() => edit(null)}>新建计划</button></div></header>
    {!enabled && <p className="subscription-empty">{isDesktop ? "订阅视图暂时暂停，待当前来源操作完成后重新读取。" : "订阅与计划需要桌面端本地账本。浏览器预览不会读取账号或保存计划。"}</p>}
    {enabled && privacyMode && <p className="subscription-empty">隐私模式：仅展示 Core 提供的安全汇总，计划与容量依据编辑已关闭，后台计量继续。</p>}
    {domainMatches && error && <p className="subscription-error" role="alert">{error}{visibleOverview ? " 当前展示保留上一次读取结果。" : ""}</p>}
    {domainMatches && message && <p className="subscription-message" role="status">{message}</p>}
    {!overview && enabled && loading && <p className="subscription-empty" role="status">正在读取本地订阅视图…</p>}
    {visibleOverview && overview && <>
      {editor && <PlanEditor key={editor.key} overview={overview} existing={editor.plan} draft={editor.draft} disabled={disabled} onSave={(input) => savePlan(input, editor.scopeToken)} onCancel={() => { setEditor(null); setDraft(null); }} />}
      {draft && <UsagePlanDraftBasis draft={draft} />}
      {basisOpen && <CapacityAssumptionEditor key={editingBasis ? `${editingBasis.id}:${editingBasis.revision}` : "new"} overview={overview} existing={editingBasis} disabled={disabled} onSave={saveAssumption} onCancel={() => setBasisOpen(false)} />}
      <SubscriptionView overview={overview} disabled={disabled} onEditPlan={edit} onToggleReminders={(plan) => { void status(plan); }} onEndPlan={(plan) => { void status(plan, true); }} onEditAssumption={(assumption) => { setEditingBasis(assumption); setBasisScope(overview.scopeToken); setBasisOpen(true); setEditor(null); }} />
      <div className="subscription-inline-actions subscription-bottom-actions"><button type="button" disabled={disabled || !overview.capabilities.capacityAssumptions || !overview.windows.length} onClick={() => { setEditingBasis(null); setBasisScope(overview.scopeToken); setBasisOpen(true); setEditor(null); }}>添加手动容量依据</button></div>
      {overview.capabilities.drafts && <section className="subscription-section" aria-labelledby="subscription-draft-title"><h3 id="subscription-draft-title">本地计划草案</h3><p className="subscription-note">由本地规则生成，不调用额外模型；草案只回填编辑器，保存后才启用。</p><div className="subscription-form-grid">
        <label>相关窗口<select value={draftWindow} disabled={disabled} onChange={(event) => setDraftWindow(event.target.value)}><option value="">未指定窗口（仅历史需求）</option>{overview.windows.filter((window) => window.canBindPlan).map((window) => <option key={window.id} value={window.id}>{window.bucketName} · {window.label}</option>)}</select></label>
        <label>模型口径<select value={draftDimension} disabled={disabled} onChange={(event) => { setDraftDimension(event.target.value as "response" | "request"); setDraftModel(""); }}><option value="response">响应模型</option><option value="request">请求模型</option></select></label>
        <label>模型范围<select value={draftModel} disabled={disabled} onChange={(event) => setDraftModel(event.target.value)}><option value="">当前来源全部模型</option>{[...new Set(overview.modelUsage.filter((usage) => usage.modelDimension === draftDimension && usage.model && usage.model !== "unknown").map((usage) => usage.model!))].map((model) => <option key={model} value={model}>{model}</option>)}</select></label>
        <label>余量预留（%）<input type="number" min="0" max="99.99" step="any" value={reserve} disabled={disabled} onChange={(event) => setReserve(event.target.value)} /></label>
      </div><div className="subscription-inline-actions"><button type="button" disabled={disabled || !overview.source} onClick={() => { void generate("remaining"); }}>按当前余量生成</button><button type="button" disabled={disabled || !overview.source} onClick={() => { void generate("habit"); }}>按历史习惯生成</button></div></section>}
    </>}
  </section>;
}
