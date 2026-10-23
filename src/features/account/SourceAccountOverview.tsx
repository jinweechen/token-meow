import { useCallback, useEffect, useRef, useState } from "react";
import { isDesktop } from "../../shared/bridge";
import type { SourceInstance } from "../../shared/contracts";
import { dateTime } from "../../shared/format";
import { sourceKindName } from "../../shared/sourceKind";
import { SourceAccountsReadModel, sourceAccountsClient, sourceAccountStatus,
  type SourceAccountRow, type SourceAccountStatus, type SourceAccountsView } from "../../shared/sourceAccounts";
import { Badge, Icon } from "../../shared/UI";
import "./sourceAccounts.css";

const labels: Record<SourceAccountStatus, string> = { not_read: "尚未读取", ready: "已读取", stale: "上次读数已过期",
  unsupported: "官方账户读取未支持", disabled: "来源已停用", unavailable: "暂不可用" };
const empty = (): SourceAccountsView => ({ overview: null, pending: false, refreshingSourceId: null, error: null, rowErrors: {} });

function useSourceAccounts(sources: SourceInstance[], privacyMode: boolean, scopeKey: string, enabled: boolean) {
  const domain = JSON.stringify([scopeKey, enabled, privacyMode, sources.map(source => [source.id, source.clientKind, source.enabled, source.root])]);
  const currentDomain = useRef(domain); currentDomain.current = domain;
  const owner = useRef<{ domain: string; value: SourceAccountsReadModel } | null>(null);
  const [accepted, setAccepted] = useState<{ domain: string; view: SourceAccountsView }>(() => ({ domain, view: empty() }));
  const [, setClock] = useState(0);
  useEffect(() => {
    setAccepted({ domain, view: empty() });
    if (!enabled || !isDesktop) return;
    const value = new SourceAccountsReadModel(sourceAccountsClient, sources, view => {
      if (currentDomain.current === domain) setAccepted({ domain, view });
    });
    const session = { domain, value }; owner.current = session; value.start();
    return () => { value.dispose(); if (owner.current === session) owner.current = null; };
  }, [domain, enabled]);
  useEffect(() => {
    if (!enabled) return;
    let timer: number | undefined;
    const update = () => {
      if (timer !== undefined) { clearInterval(timer); timer = undefined; }
      setClock(value => value + 1);
      // 这里只更新过期标记，不自动发起账户 RPC。
      if (!document.hidden) timer = window.setInterval(() => setClock(value => value + 1), 1000);
    };
    update(); document.addEventListener("visibilitychange", update);
    return () => { if (timer !== undefined) clearInterval(timer); document.removeEventListener("visibilitychange", update); };
  }, [enabled]);
  const refresh = useCallback((sourceId: string) => {
    const session = owner.current;
    return session?.domain === domain && currentDomain.current === domain ? session.value.refresh(sourceId) : Promise.resolve(false);
  }, [domain]);
  const reload = useCallback(() => { if (owner.current?.domain === domain && currentDomain.current === domain) owner.current.value.reload(); }, [domain]);
  return { view: accepted.domain === domain && enabled ? accepted.view : empty(), now: Date.now(), refresh, reload };
}

function SourceQuota({ row, stale }: { row: SourceAccountRow; stale: boolean }) {
  const buckets = row.snapshot?.buckets ?? [];
  if (!buckets.length) return <p className="subtle-note">官方额度读数尚未提供，不按 0 计算。</p>;
  return <div className="source-account-buckets">
    {stale && <p className="subtle-note">保留上次读数；尚未确认额度恢复。</p>}
    {buckets.map(bucket => <div key={bucket.id} className="source-account-bucket">
      <strong>{bucket.name}</strong>
      {([bucket.primary, bucket.secondary] as const).map((window, index) => window ? <div key={index} className="source-account-window">
        <span>{window.windowDurationMins == null ? "窗口长度未提供" : window.windowDurationMins >= 1440
          ? `${window.windowDurationMins / 1440} 天窗口` : `${window.windowDurationMins / 60} 小时窗口`}</span>
        <strong>{window.usedPercent == null ? "已用比例未提供" : `${window.usedPercent}% 已用`}</strong>
        <small>重置 {dateTime(window.resetsAt)}</small>
      </div> : null)}
      {!bucket.primary && !bucket.secondary && <p className="subtle-note">窗口读数未提供。</p>}
    </div>)}
  </div>;
}

export function SourceAccountOverview({ sources, privacyMode, scopeKey, enabled }: {
  sources: SourceInstance[]; privacyMode: boolean; scopeKey: string; enabled: boolean;
}) {
  const { view, now, refresh, reload } = useSourceAccounts(sources, privacyMode, scopeKey, enabled);
  if (!enabled) return null;
  return <section className="card source-account-overview" aria-label="逐来源账户快照">
    <div className="section-heading"><h2><Icon name="shield" size={18} />逐来源账户快照</h2>
      <button type="button" className="button secondary small" disabled={view.pending || !isDesktop} onClick={reload}>重读状态</button></div>
    <p>已登记 {sources.length} 个来源 · 已确认账号数量：未知。不同目录不代表不同账号，额度百分比分别展示，不合计。</p>
    {!isDesktop && <p className="subtle-note">浏览器预览不读取本机账户；请在桌面应用查看。</p>}
    {view.error && <p className="notice inset" role="status">{privacyMode ? "来源账户读取未成功，详细提示已隐藏。" : view.error}</p>}
    {view.pending && !view.overview && <p className="subtle-note" role="status">正在读取当前来源的缓存状态…</p>}
    {!sources.length && <p className="subtle-note">尚未登记来源；可在“来源与健康”登记工具数据目录。</p>}
    {sources.map((source, index) => {
      const row = view.overview?.rows.find(row => row.sourceId === source.id && row.clientKind === source.clientKind && row.enabled === source.enabled);
      const status = !source.enabled ? "disabled" : source.clientKind !== "codex" ? "unsupported" : row ? sourceAccountStatus(row, now) : "not_read";
      const canRefresh = isDesktop && source.enabled && source.clientKind === "codex" && !!row && status !== "unsupported" && status !== "disabled";
      const rowError = Object.hasOwn(view.rowErrors, source.id) ? view.rowErrors[source.id] : null;
      const localAttempt = view.refreshingSourceId === source.id;
      return <article key={source.id} className="source-account-row" aria-label={`${sourceKindName(source.clientKind)} 来源 ${index + 1} 账户快照`}>
        <div className="source-account-heading"><h3>{sourceKindName(source.clientKind)} · 来源 {index + 1}{row?.primary && <Badge>主来源</Badge>}</h3>
          <Badge tone={status === "ready" ? "mint" : "amber"}>{labels[status]}</Badge>
          {canRefresh && <button type="button" className="button secondary small" disabled={view.pending}
            onClick={() => { void refresh(source.id); }} aria-label={`刷新来源 ${index + 1} 的账户快照`}>{localAttempt ? "正在读取…" : "刷新此来源"}</button>}
        </div>
        <p className="wrap-path">{privacyMode ? "来源路径已隐藏" : source.root}</p>
        <p className="subtle-note">{row?.identityStatus === "unproven" ? "账号归属未证实" : "账号身份未知"}；来源标识不作为账号身份。</p>
        {source.clientKind === "codex" && row?.snapshot && (status === "ready" || status === "stale")
          ? <SourceQuota row={row} stale={status === "stale"} />
          : <p className="subtle-note">{status === "unsupported" ? "仅有本地日志用量，尚未接入此工具的官方账户额度。" : status === "disabled" ? "停用来源不会发起账户刷新。" : "尚无可展示的官方额度读数。"}</p>}
        {rowError && <p className="subtle-note" role="status">{privacyMode ? "刷新失败，详细提示已隐藏。" : `刷新失败：${rowError}`}</p>}
        {!!row?.reasons.length && <p className="subtle-note">{privacyMode ? "读取提示已隐藏。" : row.reasons.join("；")}</p>}
        <p className="subtle-note">采集于 {dateTime(row?.snapshot?.collectedAt)} · 最近尝试 {dateTime(row?.lastAttemptAt)}</p>
      </article>;
    })}
    {view.overview && <p className="subtle-note">状态读取于 {dateTime(view.overview.asOf)}；缓存重读不等于重新取得官方额度。</p>}
  </section>;
}
