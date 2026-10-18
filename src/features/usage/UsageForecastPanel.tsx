import { useState } from "react";
import type { Query, UsageDashboardData } from "../../shared/contracts";
import {
  calculateUsageForecasts,
  type ForecastDayHorizon,
  type ForecastTurnHorizon,
  type UnavailableForecast,
} from "../../shared/usageForecast";
import { usageNumber, usageTime } from "../../shared/usageDashboard";
import "./usageForecast.css";

interface Props {
  data: UsageDashboardData;
  query: Query;
  blockedReason?: string | null;
}

function Unavailable({
  value,
  blockedReason,
}: {
  value: UnavailableForecast;
  blockedReason?: string | null;
}) {
  const reasons: Record<UnavailableForecast["reason"], string> = {
    blocked: blockedReason || "等待采集完成后再预测。",
    invalid_as_of: "统计基准时间不可用，刷新账本后重试。",
    invalid_date_range: "日期范围无效，请重新选择日期。",
    missing_basis: "预测样本尚未提供，请更新并导入本机账本。",
    today_outside_range: "当前日期筛选不含今天，可切换为今天或包含今天的范围。",
    today_missing: "今天尚无可信的已记录用量，暂不外推。",
    early_today: "上海时间今天尚未经过 3 小时，暂不外推。",
    too_few_responses: "今天至少需要 3 条可信响应，暂不外推。",
    too_few_days: `最近 7 个已结束日中只有 ${value.sampleCount} 天有效样本，至少需要 3 天。`,
    select_session: "选择一个会话后，可根据该会话的完整回合预测。",
    select_source: "会话来源尚未确定，请从会话统计选择具体来源下的会话。",
    too_few_turns: `当前筛选会话只有 ${value.sampleCount} 个完整回合样本，至少需要 3 个。`,
    invalid_horizon: "预测范围无效，请重新选择。",
    numeric_overflow: "用量超出可安全计算范围，暂不生成预测。",
  };
  return (
    <div className="usage-forecast-unavailable">
      <strong>暂不预测</strong>
      <p>{reasons[value.reason]}</p>
    </div>
  );
}

function Amount({
  value,
  className = "",
}: {
  value: number;
  className?: string;
}) {
  return (
    <strong
      className={`usage-forecast-value ${className}`}
      title={`${usageNumber(value)} Tokens`}
    >
      ≈ {usageNumber(value, true)}
      <small> Tokens</small>
    </strong>
  );
}

function Range({ low, high }: { low: number; high: number }) {
  return (
    <p className="usage-forecast-range">
      历史低 / 高场景{" "}
      <span title={`${usageNumber(low)} – ${usageNumber(high)} Tokens`}>
        {usageNumber(low, true)} – {usageNumber(high, true)}
      </span>
    </p>
  );
}

export function UsageForecastPanel({ data, query, blockedReason }: Props) {
  const [dayHorizon, setDayHorizon] = useState<ForecastDayHorizon>(7);
  const [turnHorizon, setTurnHorizon] = useState<ForecastTurnHorizon>(5);
  // Recompute from each snapshot/query; a previous project's samples are never cached.
  const forecast = calculateUsageForecasts(data, query, {
    dayHorizon,
    turnHorizon,
    blockedReason,
  });
  const needsSessionSelection =
    forecast.turns.status === "unavailable" &&
    (forecast.turns.reason === "select_session" ||
      forecast.turns.reason === "select_source");
  return (
    <section className="usage-forecast" aria-labelledby="usage-forecast-title">
      <div className="usage-forecast-heading">
        <div>
          <h2 id="usage-forecast-title">
            Token 预测 <span>本地历史估算</span>
          </h2>
          <p>按当前筛选范围的已记录用量外推，数据不足时不生成预测。</p>
        </div>
        <span className="usage-forecast-clock">
          基准 {usageTime(data.asOf, true)} · 上海时区
        </span>
      </div>
      <div
        className={`usage-forecast-grid${needsSessionSelection ? " usage-forecast-grid-general" : ""}`}
      >
        <article className="usage-forecast-card">
          <div className="usage-forecast-card-heading">
            <h3>今日预计总量</h3>
            <span>截至 24:00</span>
          </div>
          {forecast.today.status === "ready" ? (
            <>
              <Amount value={forecast.today.estimatedTokens} />
              <p className="usage-forecast-range">
                已记录{" "}
                <span>{usageNumber(forecast.today.observedTokens, true)}</span>{" "}
                · 剩余预计{" "}
                <span>{usageNumber(forecast.today.remainingTokens, true)}</span>
              </p>
              <details className="usage-forecast-method-details">
                <summary>估算方法</summary>
                <p className="usage-forecast-method">
                  按已过 {forecast.today.elapsedHours.toFixed(1)}{" "}
                  小时的平均速率延续至当天结束。
                </p>
              </details>
              <p className="usage-forecast-samples">
                {forecast.todayDate} ·{" "}
                {usageNumber(forecast.today.responseCount)} 条可信响应
              </p>
            </>
          ) : (
            <Unavailable value={forecast.today} blockedReason={blockedReason} />
          )}
        </article>
        <article className="usage-forecast-card">
          <div className="usage-forecast-card-heading">
            <h3>未来 {dayHorizon} 天新增</h3>
            <div
              className="usage-forecast-switch"
              role="group"
              aria-label="未来预测天数"
            >
              {([1, 3, 7] as const).map((days) => (
                <button
                  type="button"
                  key={days}
                  className={dayHorizon === days ? "active" : ""}
                  aria-pressed={dayHorizon === days}
                  onClick={() => setDayHorizon(days)}
                >
                  {days} 天
                </button>
              ))}
            </div>
          </div>
          {forecast.future.status === "ready" ? (
            <>
              <Amount value={forecast.future.estimatedTokens} />
              <Range
                low={forecast.future.lowTokens}
                high={forecast.future.highTokens}
              />
              <details className="usage-forecast-method-details">
                <summary>估算方法与范围</summary>
                <p className="usage-forecast-method">
                  假设每天保持历史有记录日的使用强度。低 /
                  高场景按历史用量日估算，实际用量可能超出范围。
                </p>
              </details>
              <p
                className="usage-forecast-samples"
                title={`有效样本日期：${forecast.future.sampleKeys.join("、")}`}
              >
                {forecast.historyFrom} 至 {forecast.historyTo} ·{" "}
                {forecast.future.sampleCount} / 7 天有记录
              </p>
            </>
          ) : (
            <Unavailable
              value={forecast.future}
              blockedReason={blockedReason}
            />
          )}
          {forecast.futureFrom && forecast.futureTo && (
            <p className="usage-forecast-period">
              预测日期 {forecast.futureFrom} 至 {forecast.futureTo}
            </p>
          )}
        </article>
        {!needsSessionSelection && (
          <article className="usage-forecast-card">
            <div className="usage-forecast-card-heading">
              <h3>会话后续回合</h3>
              <div
                className="usage-forecast-switch"
                role="group"
                aria-label="会话预测回合数"
              >
                {([1, 5, 10] as const).map((turns) => (
                  <button
                    type="button"
                    key={turns}
                    className={turnHorizon === turns ? "active" : ""}
                    aria-pressed={turnHorizon === turns}
                    onClick={() => setTurnHorizon(turns)}
                  >
                    {turns} 回合
                  </button>
                ))}
              </div>
            </div>
            {forecast.turns.status === "ready" ? (
              <>
                <Amount value={forecast.turns.estimatedTokens} />
                <Range
                  low={forecast.turns.lowTokens}
                  high={forecast.turns.highTokens}
                />
                <p className="usage-forecast-cumulative">
                  当前筛选会话累计{" "}
                  <span>
                    {forecast.turns.currentTokens == null
                      ? "未提供"
                      : usageNumber(forecast.turns.currentTokens, true)}
                  </span>
                  <br />
                  加上预测后累计{" "}
                  <strong
                    title={
                      forecast.turns.cumulativeTokens == null
                        ? undefined
                        : `${usageNumber(forecast.turns.cumulativeTokens)} Tokens`
                    }
                  >
                    {forecast.turns.cumulativeTokens == null
                      ? "暂不可计算"
                      : `≈ ${usageNumber(forecast.turns.cumulativeTokens, true)}`}
                  </strong>
                </p>
                <details className="usage-forecast-method-details">
                  <summary>估算方法与范围</summary>
                  <p className="usage-forecast-method">
                    按最近 {forecast.turns.sampleCount}{" "}
                    个完整回合的平均用量，估算后续 {turnHorizon}{" "}
                    回合。实际用量可能超出历史场景。
                  </p>
                </details>
                {forecast.turns.cumulativeUnavailable && (
                  <p className="usage-forecast-method">
                    {forecast.turns.cumulativeUnavailable === "missing_current"
                      ? "当前累计用量缺失，只显示新增预测。"
                      : "累计值超出可安全计算范围，只显示新增预测。"}
                  </p>
                )}
                <p className="usage-forecast-samples">
                  样本 {usageTime(forecast.turns.sampleFrom, true)} 至{" "}
                  {usageTime(forecast.turns.sampleTo, true)}
                  <br />
                  完整回合 {forecast.turns.sampleCount} 个 · 未完成或不完整回合{" "}
                  {forecast.omittedIncompleteTurns == null
                    ? "数量未知"
                    : `${forecast.omittedIncompleteTurns} 个未纳入`}
                </p>
              </>
            ) : (
              <Unavailable
                value={forecast.turns}
                blockedReason={blockedReason}
              />
            )}
          </article>
        )}
      </div>
      {needsSessionSelection && forecast.turns.status === "unavailable" && (
        <div className="usage-forecast-session-guide">
          <strong>会话后续回合</strong>
          <Unavailable value={forecast.turns} blockedReason={blockedReason} />
        </div>
      )}
      <p className="usage-forecast-note">
        Tokens
        为累计消耗（含缓存），不表示上下文窗口占用；预测受本地采集覆盖和后续使用强度影响，不是官方额度或账单。
      </p>
    </section>
  );
}
