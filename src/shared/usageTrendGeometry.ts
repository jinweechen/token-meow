export type UsageChartType = "bar" | "line" | "area";

export const USAGE_CHART_TYPE_KEY = "token-meow.usage-chart-type";

export function isUsageChartType(value: unknown): value is UsageChartType {
  return value === "bar" || value === "line" || value === "area";
}

/** Missing, malformed and negative usage must never become an observed zero. */
export function usageTrendValue(
  value: string | number | null | undefined,
): number | null {
  if (value == null) return null;
  if (typeof value === "string" && !/^\d+(?:\.\d+)?$/.test(value.trim()))
    return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : null;
}

export interface UsageTrendCoordinate {
  index: number;
  value: number;
  x: number;
  y: number;
}

/** Segments stop at unknown buckets, so a line never implies measured continuity. */
export function usageTrendGeometry(
  values: readonly (number | null)[],
  plot: { left: number; top: number; width: number; height: number },
) {
  const validValues = values.map(usageTrendValue);
  const max = Math.max(0, ...validValues.map((value) => value ?? 0)) || 1;
  const step = plot.width / Math.max(1, values.length);
  const coordinates = validValues.map((value, index) =>
    value == null
      ? null
      : {
          index,
          value,
          x: plot.left + (index + 0.5) * step,
          y: plot.top + plot.height * (1 - value / max),
        },
  );
  const segments: UsageTrendCoordinate[][] = [];
  let current: UsageTrendCoordinate[] = [];
  for (const coordinate of coordinates) {
    if (coordinate) current.push(coordinate);
    else if (current.length) {
      segments.push(current);
      current = [];
    }
  }
  if (current.length) segments.push(current);
  return { max, step, coordinates, segments };
}
