import type { AccountSnapshot } from "./contracts";

export function officialUsageDays(account: AccountSnapshot | undefined) {
  const buckets = account?.usage?.dailyUsageBuckets;
  if (buckets == null) return null;
  return buckets
    .filter((point) => {
      if (!/^\d{4}-\d{2}-\d{2}$/.test(point.startDate)) return false;
      const value = new Date(`${point.startDate}T00:00:00Z`);
      return (
        Number.isFinite(value.getTime()) &&
        value.toISOString().slice(0, 10) === point.startDate
      );
    })
    .map((point) => ({
      startDate: point.startDate,
      tokens:
        point.tokens != null &&
        Number.isFinite(point.tokens) &&
        point.tokens >= 0
          ? point.tokens
          : null,
    }))
    .sort((left, right) => left.startDate.localeCompare(right.startDate));
}
