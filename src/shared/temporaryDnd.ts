import type { Settings } from "./contracts";

export function temporaryDndState(settings: Settings) {
  const value = settings.temporaryDoNotDisturb;
  if (!value || typeof value.active !== "boolean" || !Number.isSafeInteger(value.remainingMs) ||
      value.remainingMs < 0 || value.remainingMs > 60 * 60_000) return null;
  if (value.active && (typeof value.until !== "string" || !Number.isFinite(Date.parse(value.until)) ||
      value.remainingMs === 0 || !Number.isSafeInteger(value.deadlineMonoMs) || Number(value.deadlineMonoMs) < 0)) return null;
  return value;
}

export function temporaryDndStatus(settings: Settings): string {
  const state = temporaryDndState(settings);
  if (!state) return "临时免打扰状态暂未提供";
  if (state.active) return `临时免打扰中，约剩 ${Math.ceil(state.remainingMs / 60_000)} 分钟`;
  return settings.effectiveDoNotDisturb
    ? "临时免打扰未开启；当前仍受手动或每日时段影响"
    : "临时免打扰未开启";
}
