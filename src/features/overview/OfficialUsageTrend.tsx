import { useState } from "react";
import type { AccountSnapshot } from "../../shared/contracts";
import { officialUsageDays } from "../../shared/accountUsage";
import { formatNumber } from "../../shared/format";

export function OfficialUsageTrend({ account }: { account?: AccountSnapshot }) {
  const [hover, setHover] = useState<number | null>(null);
  const days = officialUsageDays(account);
  const points = days?.slice(-30) ?? [];
  const maximum = Math.max(1, ...points.map((point) => point.tokens ?? 0));
  const selected = (hover != null ? points[hover] : undefined) ?? points.at(-1);
  return (
    <section className="official-trend" aria-label="官方账户日趋势">
      <div className="section-heading">
        <h3>官方账户日趋势</h3>
        <span>{days == null ? "来源未提供" : `${points.length} 个记录日`}</span>
      </div>
      {!points.length ? (
        <p className="subtle-note">
          {days == null
            ? "账户来源未提供每日用量，未按 0 绘图。"
            : "官方快照暂无每日记录。"}
        </p>
      ) : (
        <>
          <div className="official-trend-bars">
            {points.map((point, index) => (
              <button
                key={`${point.startDate}:${index}`}
                className={`official-day ${point.tokens == null ? "unknown-day" : ""}`}
                aria-label={`${point.startDate} · ${point.tokens == null ? "用量未提供" : `${formatNumber(point.tokens)} token`}`}
                title={`${point.startDate} · ${point.tokens == null ? "用量未提供" : `${formatNumber(point.tokens)} token`}`}
                onMouseEnter={() => setHover(index)}
                onFocus={() => setHover(index)}
                onMouseLeave={() => setHover(null)}
              >
                <span
                  style={{
                    height:
                      point.tokens == null
                        ? "100%"
                        : `${Math.max(2, (point.tokens / maximum) * 100)}%`,
                  }}
                />
              </button>
            ))}
          </div>
          <div className="official-trend-range">
            <span>{points[0].startDate}</span>
            <span>{points.at(-1)!.startDate}</span>
          </div>
          <p className="official-trend-value">
            {selected?.startDate}{" "}
            <strong>
              {selected?.tokens == null
                ? "用量未提供"
                : `${formatNumber(selected.tokens)} token`}
            </strong>
          </p>
        </>
      )}
      <p className="subtle-note">
        官方 startDate 原日期口径，时区未单独提供。显示最近 30
        个记录日，缺日不补零，斜纹表示用量未提供；不随本机日期筛选，也不与本机账本相加。
      </p>
    </section>
  );
}
