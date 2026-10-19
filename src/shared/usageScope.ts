import type { Query, Snapshot, UsageDashboardScopeGroup } from "./contracts";

/** Drill into a row as an intersection of the visible filters, not a new scope. */
export function usageScopeQuery(
  query: Query,
  kind: "projects" | "sessions",
  row: Pick<UsageDashboardScopeGroup, "sourceId" | "projectId" | "threadId">,
): Query | null {
  if (kind === "projects") {
    if (!row.projectId) return null;
    return {
      ...query,
      project: row.projectId,
      ...(row.sourceId ? { sourceId: row.sourceId } : {}),
    };
  }
  if (!row.sourceId || !row.threadId) return null;
  return { ...query, sourceId: row.sourceId, threadId: row.threadId };
}

export interface UsageCollectionCoverage {
  title: string;
  notes: string[];
  attention: boolean;
}

const count = (value: unknown): number | null =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
const problems = (value: unknown) => Array.isArray(value) ? value.length : count(value);
const successful = (value: unknown) => typeof value === "string" && Number.isFinite(Date.parse(value));

/** Collection facts explain partial totals; they do not certify exhaustive coverage.
 * Global pending/history counts span all enabled sources, so a source filter must
 * use that source's own check instead. Paused/cancelled are global HISTORY flags,
 * not an assertion that live collection has stopped. Forecast rules stay separate.
 */
export function usageCollectionCoverage(
  snapshot: Pick<Snapshot, "health" | "sources">,
  query: Query,
): UsageCollectionCoverage {
  const health = snapshot.health;
  const history = health.history;
  const filtered = !!(query.sourceId || query.clientKind);
  const selected = snapshot.sources?.filter((source) =>
    (!query.sourceId || source.id === query.sourceId) &&
    (!query.clientKind || source.clientKind === query.clientKind));
  const enabled = selected?.filter((source) => source.enabled);
  const canCollect = enabled == null || enabled.length > 0;
  const pending = (enabled ?? []).map((source) => count(source.health?.pendingFiles));
  const sourcePending = pending.reduce<number>((sum, value) => sum + (value ?? 0), 0);
  // Counts can describe overlapping work. Never add global/history to sources.
  const knownPending = Math.max(Number.isSafeInteger(sourcePending) ? sourcePending : 0,
    ...(!filtered && canCollect ? [count(health.pendingFiles) ?? 0, count(history?.pendingFiles) ?? 0] : []));
  const paused = history?.status === "paused";
  const cancelled = history?.status === "cancelled";
  const importing = canCollect && (knownPending > 0 || pending.some((value) => value != null && value > 0) ||
    (!filtered && history?.status === "running"));
  const waiting = (selected != null && selected.length === 0) ||
    (!filtered && !successful(health.lastSuccessfulScan)) ||
    !!enabled?.some((source) => !successful(source.health?.lastSuccessfulCheck));
  const collectionIssues = !!selected?.some((source) => source.runtime === "不可用" ||
    (problems(source.health?.issues) ?? 0) > 0 || (count(source.health?.unsupportedFiles) ?? 0) > 0) ||
    (!filtered && ((problems(health.issues) ?? 0) > 0 || (count(health.unsupportedFiles) ?? 0) > 0));
  const disabled = !!selected?.some((source) => !source.enabled);
  const unknown = selected == null || selected.length === 0 || !Number.isSafeInteger(sourcePending) || !!enabled?.some((source) =>
    !["可用", "不可用"].includes(source.runtime ?? "") || count(source.health?.pendingFiles) === null ||
    problems(source.health?.issues) === null) ||
    (!filtered && (count(health.pendingFiles) === null || count(history?.pendingFiles) === null ||
      !history || !["running", "paused", "cancelled", "idle"].includes(history.status)));
  const notes: string[] = [];
  if (paused) notes.push("暂停仅作用于历史补导，不会因此停止实时新增记录的采集。");
  else if (cancelled) notes.push("取消仅作用于本次历史补导，不会因此停止实时新增记录的采集。");
  if (knownPending > 0) notes.push(`已知仍有 ${knownPending.toLocaleString("zh-CN")} 个文件待采集。`);
  if (waiting) notes.push("所选范围尚无成功采集检查记录。");
  if (collectionIssues) notes.push("来源存在采集、解析或不支持的文件问题，部分记录可能未计入。");
  if (disabled) notes.push("有来源已停用，仅保留此前已采集的记录。");
  if (unknown) notes.push("部分采集状态尚未提供，不能据此判断覆盖完整。");
  const title = paused ? "历史补导已暂停" : cancelled ? "历史补导已取消"
    : importing ? "日志正在导入，当前统计尚不完整"
    : waiting ? "等待成功采集检查" : collectionIssues ? "部分记录需要检查"
    : disabled ? "所选范围包含已停用来源" : unknown ? "采集覆盖状态尚未确认"
    : "统计仅含已采集记录";
  return { title, notes, attention: notes.length > 0 || importing };
}

/** Forecasts must not treat an unfinished or unreliable collection as usage pace. */
export function usageForecastBlockedReason(
  snapshot: Pick<Snapshot, "health" | "sources" | "usageDashboard">,
  query: Query,
): string | null {
  const health = snapshot.health;
  const history = health.history;
  if (
    Number(health.pendingFiles) > 0 ||
    Number(history?.pendingFiles) > 0 ||
    (history != null && history.status !== "idle")
  )
    return "日志导入尚未完成，完成采集后再计算预测。";
  if (!health.lastSuccessfulScan) return "尚无成功采集记录，暂时无法预测。";
  if ((snapshot.usageDashboard?.summary.excludedResponses ?? 0) > 0)
    return "当前范围存在计量缺失或冲突，处理后再计算预测。";
  const sources = (snapshot.sources ?? []).filter(
    (source) =>
      source.enabled &&
      (!query.sourceId || source.id === query.sourceId) &&
      (!query.clientKind || source.clientKind === query.clientKind),
  );
  if (sources.some((source) => Number(source.health?.pendingFiles) > 0))
    return "所选来源仍有日志待采集，暂时无法预测。";
  if (sources.some((source) => Number(source.health?.issues) > 0))
    return "所选来源存在采集问题，请先查看来源与数据健康。";
  return null;
}
