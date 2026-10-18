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
