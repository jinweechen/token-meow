import { useState } from "react";
import type { UsageDashboardData } from "../../shared/contracts";
import { usageNumber, usageUsd } from "../../shared/usageDashboard";
import {
  isUsageChartType,
  USAGE_CHART_TYPE_KEY,
  usageTrendGeometry,
  usageTrendValue,
  type UsageChartType,
} from "../../shared/usageTrendGeometry";

type Metric = "tokens" | "responses" | "cost";
const metrics = [
  ["tokens", "Tokens"],
  ["responses", "响应"],
  ["cost", "估算成本"],
] as const;
const chartTypes = [
  ["bar", "柱状图"],
  ["line", "折线图"],
  ["area", "面积图"],
] as const;

function savedChartType(): UsageChartType {
  try {
    const saved = window.localStorage.getItem(USAGE_CHART_TYPE_KEY);
    if (isUsageChartType(saved)) return saved;
  } catch {
    // Storage may be unavailable; chart switching still works for this session.
  }
  return "bar";
}

export function UsageTrend({ data }: { data: UsageDashboardData }) {
  const [metric, setMetric] = useState<Metric>("tokens");
  const [chartType, setChartType] = useState<UsageChartType>(savedChartType);
  const [hoveredBucket, setHoveredBucket] = useState<string | null>(null);
  const points = data.trend;
  const hoveredIndex = points.findIndex(
    (point) => point.bucket === hoveredBucket,
  );
  const hover = hoveredIndex >= 0 ? hoveredIndex : null;
  const values = points.map((point) =>
    usageTrendValue(
      metric === "tokens"
        ? point.totalTokens
        : metric === "responses"
          ? point.responseCount
          : point.estimatedCostUsd,
    ),
  );
  const width = Math.max(760, points.length * 15),
    height = 180,
    padding = 56,
    plotWidth = width - padding - 12,
    plotHeight = 124;
  const baseline = 14 + plotHeight;
  const { max, step, coordinates, segments } = usageTrendGeometry(values, {
    left: padding,
    top: 14,
    width: plotWidth,
    height: plotHeight,
  });
  const barWidth = Math.max(1, Math.min(36, step * 0.72));
  const labelEvery = Math.max(1, Math.ceil(points.length / 10));
  const valueLabel = (index: number) =>
    metric === "cost"
      ? values[index] == null && points[index].estimatedCostUsd != null
        ? "未提供"
        : usageUsd(points[index].estimatedCostUsd)
      : usageNumber(values[index]);
  const hovered = hover != null ? points[hover] : null;
  const chartLabel = chartTypes.find(([key]) => key === chartType)![1];
  return (
    <section className="usage-trend" aria-labelledby="usage-trend-title">
      <div className="usage-section-heading">
        <h2 id="usage-trend-title">
          使用趋势{" "}
          <span>
            · 按{data.granularity === "hour" ? "小时" : "天"} · 上海时区
          </span>
        </h2>
        <div className="usage-trend-controls">
          <div className="usage-segment" role="group" aria-label="趋势指标">
            {metrics.map(([key, label]) => (
              <button
                key={key}
                type="button"
                aria-pressed={metric === key}
                className={metric === key ? "active" : ""}
                onClick={() => {
                  setMetric(key);
                  setHoveredBucket(null);
                }}
              >
                {label}
              </button>
            ))}
          </div>
          <div
            className="usage-segment usage-chart-types"
            role="group"
            aria-label="图表类型"
          >
            {chartTypes.map(([key, label]) => (
              <button
                key={key}
                type="button"
                aria-pressed={chartType === key}
                className={chartType === key ? "active" : ""}
                onClick={() => {
                  setChartType(key);
                  setHoveredBucket(null);
                  try {
                    window.localStorage.setItem(USAGE_CHART_TYPE_KEY, key);
                  } catch {
                    // A denied preference write must not block chart interactions.
                  }
                }}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
      </div>
      {points.length ? (
        <>
          <div className="usage-chart-scroll">
            <svg
              className="usage-chart"
              viewBox={`0 0 ${width} ${height}`}
              style={{ minWidth: width }}
              role="img"
              data-chart-type={chartType}
              aria-label={`${chartLabel}，按${data.granularity === "hour" ? "小时" : "天"}显示${metric === "tokens" ? "Token" : metric === "responses" ? "响应数" : "已定价响应的估算成本"}，共 ${points.length} 个时间桶`}
            >
              {[0, 0.5, 1].map((fraction) => {
                const y = 14 + plotHeight * (1 - fraction);
                return (
                  <g key={fraction}>
                    <line
                      x1={padding}
                      y1={y}
                      x2={width - 12}
                      y2={y}
                      className="usage-gridline"
                    />
                    <text
                      x={padding - 9}
                      y={y + 4}
                      textAnchor="end"
                      className="usage-chart-label"
                    >
                      {metric === "cost"
                        ? usageUsd(String(max * fraction))
                        : usageNumber(max * fraction, true)}
                    </text>
                  </g>
                );
              })}
              {chartType !== "bar" &&
                segments.map((segment) => {
                  const first = segment[0],
                    last = segment[segment.length - 1];
                  const line = segment
                    .map(
                      (point, index) =>
                        `${index ? "L" : "M"} ${point.x} ${point.y}`,
                    )
                    .join(" ");
                  return (
                    <g
                      key={first.index}
                      aria-hidden="true"
                      pointerEvents="none"
                    >
                      {chartType === "area" && segment.length > 1 && (
                        <path
                          d={`M ${first.x} ${baseline} L ${first.x} ${first.y} ${segment
                            .slice(1)
                            .map((point) => `L ${point.x} ${point.y}`)
                            .join(" ")} L ${last.x} ${baseline} Z`}
                          className="usage-area-fill"
                        />
                      )}
                      <path d={line} className="usage-line-path" />
                    </g>
                  );
                })}
              {points.map((point, index) => {
                const value = values[index],
                  barHeight =
                    value != null ? Math.max(0, (value / max) * plotHeight) : 0;
                const x = padding + index * step + (step - barWidth) / 2;
                return (
                  <g
                    key={point.bucket}
                    className="usage-chart-bar"
                    tabIndex={0}
                    role="img"
                    aria-label={`${point.bucket}，${valueLabel(index)}${metric === "cost" ? "，仅包含已定价响应" : ""}`}
                    onMouseEnter={() => setHoveredBucket(point.bucket)}
                    onMouseLeave={() => setHoveredBucket(null)}
                    onFocus={() => setHoveredBucket(point.bucket)}
                    onBlur={() => setHoveredBucket(null)}
                  >
                    <title>
                      {point.bucket} · {valueLabel(index)}
                      {metric === "cost"
                        ? ` · ${point.pricedResponses} 条已定价响应`
                        : ""}
                    </title>
                    <rect
                      x={padding + index * step}
                      y="5"
                      width={step}
                      height={plotHeight + 18}
                      fill="transparent"
                    />
                    {chartType === "bar" ? (
                      <rect
                        x={x}
                        y={14 + plotHeight - Math.max(2, barHeight)}
                        width={barWidth}
                        height={Math.max(2, barHeight)}
                        rx="2"
                        className={
                          value == null
                            ? "usage-bar-missing"
                            : value === 0
                              ? "usage-bar-zero"
                              : "usage-bar-fill"
                        }
                      />
                    ) : (
                      <circle
                        cx={padding + (index + 0.5) * step}
                        cy={coordinates[index]?.y ?? baseline}
                        r={hover === index ? 4.5 : 3}
                        className={`${value == null ? "usage-point-missing" : `usage-point-marker${value === 0 ? " usage-point-zero" : ""}`}${hover === index ? " is-active" : ""}`}
                      />
                    )}
                    {(index % labelEvery === 0 ||
                      index === points.length - 1) && (
                      <text
                        x={x + barWidth / 2}
                        y={height - 16}
                        textAnchor="middle"
                        className="usage-chart-label"
                      >
                        {point.label}
                      </text>
                    )}
                  </g>
                );
              })}
            </svg>
          </div>
          <div className="usage-chart-caption" aria-live="polite">
            {hovered && hover != null ? (
              <span>
                <strong>{hovered.bucket}</strong> · {valueLabel(hover)}
                {metric === "cost" &&
                  ` · 已定价 ${hovered.pricedResponses} / ${hovered.responseCount} 条`}
              </span>
            ) : (
              <span>
                <i />
                {metric === "cost"
                  ? chartType === "bar"
                    ? "已定价响应的估算成本；灰线表示未定价"
                    : "已定价响应的估算成本；灰色空心点表示未定价，曲线在此断开"
                  : metric === "tokens"
                    ? "总 Tokens = 输入 + 输出，缓存与推理不重复累加"
                    : "已记录响应数，不等同于 HTTP 请求数或成功次数"}
              </span>
            )}
          </div>
        </>
      ) : (
        <div className="usage-empty">
          <strong>当前范围还没有用量记录</strong>
          <span>采集到有效响应后，会在这里显示趋势。</span>
        </div>
      )}
    </section>
  );
}
