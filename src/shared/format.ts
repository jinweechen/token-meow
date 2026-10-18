import type { AccountSnapshot, PetState, Snapshot, Turn } from "./contracts";

export const timeZone = "Asia/Shanghai";
export const formatNumber = (value: number | null | undefined) =>
  value == null || !Number.isFinite(value)
    ? "—"
    : new Intl.NumberFormat("zh-CN").format(value);
export const compact = (value: number | null | undefined) =>
  value == null || !Number.isFinite(value)
    ? "—"
    : value >= 1_000_000
      ? `${(value / 1_000_000).toFixed(2)}M`
      : value >= 1_000
        ? `${(value / 1_000).toFixed(1)}K`
        : String(value);
export const percent = (value: number | null | undefined) =>
  value == null || !Number.isFinite(value)
    ? "未提供"
    : `${(value * 100).toFixed(1)}%`;
export const duration = (value: number | null | undefined) =>
  value == null || value < 0 || !Number.isFinite(value)
    ? "未提供"
    : value < 1_000
      ? `${Math.round(value)} ms`
      : value < 60_000
        ? `${(value / 1_000).toFixed(1)} 秒`
        : `${Math.floor(value / 60_000)} 分 ${Math.round((value % 60_000) / 1_000)} 秒`;
export function dateTime(
  value: string | number | null | undefined,
  short = false,
): string {
  if (value == null) return "尚未记录";
  const date = new Date(typeof value === "number" ? value * 1000 : value);
  if (!Number.isFinite(date.getTime())) return "时间不可用";
  return new Intl.DateTimeFormat("zh-CN", {
    timeZone,
    ...(short ? {} : { month: "2-digit", day: "2-digit" }),
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).format(date);
}
export function localDate(date = new Date()): string {
  return new Intl.DateTimeFormat("en-CA", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).format(date);
}
export function queryDates(
  days: number,
  now = new Date(),
): { from: string; to: string } {
  const to = localDate(now);
  const date = new Date(`${to}T00:00:00+08:00`);
  date.setUTCDate(date.getUTCDate() - days + 1);
  return { from: localDate(date), to };
}
export function defaultQuery(search: string, now = new Date()) {
  return {
    ...queryDates(
      new URLSearchParams(search).get("window") === "pet" ? 1 : 7,
      now,
    ),
    includeLegacy: false,
  };
}
export function accountExpired(
  account: AccountSnapshot | undefined,
  now = Date.now(),
): boolean {
  if (!account) return false;
  const collectedAt = Date.parse(account.collectedAt ?? "");
  const hasSnapshot = account.status === "connected" || account.buckets.length > 0;
  return (
    account.status === "stale" ||
    (hasSnapshot &&
      (!Number.isFinite(now) ||
        !Number.isFinite(collectedAt) ||
        collectedAt > now ||
        now - collectedAt >= 240_000)) ||
    account.buckets.some((bucket) =>
      [bucket.primary, bucket.secondary].some(
        (window) => window?.resetsAt != null && window.resetsAt * 1000 <= now,
      ),
    )
  );
}
export const issueCount = (value: unknown[] | number | undefined) =>
  Array.isArray(value) ? value.length : (value ?? 0);
export const isSuccess = (status: string) =>
  ["completed", "success", "succeeded", "normal", "ok"].includes(
    status.toLowerCase(),
  );
export const statusLabel = (status: string) =>
  ({
    completed: "正常结束",
    success: "正常结束",
    succeeded: "正常结束",
    failed: "失败",
    error: "失败",
    interrupted: "已中断",
    cancelled: "已取消",
    canceled: "已取消",
    running: "活动中",
    active: "活动中",
    started: "活动中",
    unknown: "已结束，结果未知",
    uncertain: "状态待确认",
  })[status.toLowerCase()] ?? "已结束，结果未知";
export function latestTimedTurn(turns: Turn[]): Turn | undefined {
  const timestamp = (turn: Turn) => {
    const time = Date.parse(turn.completedAt ?? turn.startedAt ?? "");
    return Number.isFinite(time) ? time : -Infinity;
  };
  return [...turns].sort((a, b) => timestamp(b) - timestamp(a))[0];
}
export function petState(
  snapshot: Snapshot,
  latestEvent?: string | null,
): PetState {
  if (snapshot.activity.activeTurns > 0) return "active";
  if (snapshot.activity.uncertainTurns > 0) return "alert";
  if (latestEvent) return isSuccess(latestEvent) ? "complete" : "alert";
  if (snapshot.settings.doNotDisturb) return "sleep";
  return "idle";
}
