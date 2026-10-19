import { useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import type {
  Query,
  UsageDashboardData,
  UsageDashboardRequest,
  UsagePriceInput,
  UsagePriceSyncConfig,
  UsagePriceSyncConfigInput,
} from "../../shared/contracts";
import { duration, percent } from "../../shared/format";
import {
  priceReasonLabel,
  usageDay,
  usageNumber,
  usageTime,
  usageUsd,
} from "../../shared/usageDashboard";
import { UsagePricingPanel } from "./UsagePricingPanel";
import { UsageTrend } from "./UsageTrend";
import { UsageScopeTable } from "./UsageScopeTable";
import { UsageForecastPanel } from "./UsageForecastPanel";
import type { UsageCollectionCoverage } from "../../shared/usageScope";
import "./usageDashboard.css";

const tabs = [
  ["responses", "响应明细"],
  ["projects", "项目"],
  ["sessions", "会话"],
  ["providers", "供应商"],
  ["models", "模型"],
  ["pricing", "定价"],
] as const;
type Tab = (typeof tabs)[number][0];

function summaryUsd(value: string | null | undefined): string {
  if (value == null || value.trim() === "") return "未定价";
  const amount = Number(value);
  if (!Number.isFinite(amount) || amount < 0) return "未提供";
  if (amount > 0 && amount < 0.01) return "< $0.01";
  return `$${new Intl.NumberFormat("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(amount)}`;
}

function responseCoverage(value: number | null | undefined): string {
  return value == null
    ? "响应条目覆盖率未知 · 仅已提供部分"
    : value === 0
      ? "响应条目覆盖率 0.0% · 尚无已提供条目"
    : value === 1
      ? "响应条目覆盖率 100% · 全部有效响应"
      : `响应条目覆盖率 ${percent(value)} · 仅已提供部分`;
}

interface Props {
  data: UsageDashboardData | null;
  request: UsageDashboardRequest;
  query: Query;
  onSelectScope: (next: Query) => void;
  forecastBlockedReason?: string | null;
  collectionCoverage: UsageCollectionCoverage;
  onOpenHealth: () => void;
  onRequestChange: (next: UsageDashboardRequest) => void;
  loading: boolean;
  error?: string | null;
  onSavePrice: (payload: UsagePriceInput) => Promise<void>;
  onOpenThread: (threadId: string, sourceId: string) => void;
  privacyMode: boolean;
  isDemo: boolean;
  priceSyncConfig?: UsagePriceSyncConfig;
  onSavePriceSyncConfig: (input: UsagePriceSyncConfigInput) => Promise<void>;
  onSyncPricesNow: () => Promise<void>;
  priceSyncing: boolean;
}

function AggregateTable({
  rows,
  kind,
}: {
  rows: UsageDashboardData["models"];
  kind: "模型" | "供应商";
}) {
  return (
    <>
      <div className="usage-aggregate-heading">
        <span>{kind}用量汇总</span>
        <span>按有效响应数降序排列</span>
      </div>
      <div className="usage-table-scroll">
        <table className="usage-table">
          <caption className="usage-sr-only">
            按{kind}汇总当前筛选范围的有效响应用量
          </caption>
          <thead>
            <tr>
              <th>{kind}</th>
              <th className="number" aria-sort="descending">
                有效响应数 ↓
              </th>
              <th className="number">总 Tokens</th>
              <th className="number">估算成本</th>
              <th className="number">
                成功率{" "}
                <span
                  className="usage-info"
                  title="当前来源没有可归因到每条响应的结果信号，不能从已记录用量推断成功率。"
                >
                  ⓘ
                </span>
              </th>
              <th className="number">
                速度{" "}
                <span
                  className="usage-info"
                  title="缺少可靠的单响应生成计时，暂不计算 Token/s。"
                >
                  ⓘ
                </span>
              </th>
              <th className="number">
                回合首 Token{" "}
                <span
                  className="usage-info"
                  title="仅唯一可归属有效响应的回合进入均值；请结合样本覆盖率判断。不是首可见文字延迟。"
                >
                  ⓘ
                </span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.key}>
                <td>
                  <strong>
                    {!row.name || row.name === "unknown" ? "未知" : row.name}
                  </strong>
                </td>
                <td className="number">{usageNumber(row.responseCount)}</td>
                <td className="number">{usageNumber(row.totalTokens, true)}</td>
                <td
                  className={`number ${row.estimatedCostUsd == null ? "usage-unavailable" : "usage-cost"}`}
                >
                  {usageUsd(row.estimatedCostUsd)}
                  <small>
                    已定价 {row.pricedResponses} / {row.responseCount}
                  </small>
                </td>
                <td className="number usage-unavailable">未提供</td>
                <td className="number usage-unavailable">未提供</td>
                <td className="number">
                  {duration(row.turnTimeToFirstTokenMs)}
                  <small>
                    计时样本 {row.timedResponses} / {row.responseCount}
                  </small>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {!rows.length && (
        <div className="usage-empty">
          <strong>暂无{kind}统计</strong>
          <span>当前筛选范围内还没有可汇总的有效响应。</span>
        </div>
      )}
    </>
  );
}

export function UsageDashboard({
  data,
  request,
  query,
  onSelectScope,
  forecastBlockedReason,
  collectionCoverage,
  onOpenHealth,
  onRequestChange,
  loading,
  error,
  onSavePrice,
  onOpenThread,
  privacyMode,
  isDemo,
  priceSyncConfig,
  onSavePriceSyncConfig,
  onSyncPricesNow,
  priceSyncing,
}: Props) {
  const [expanded, setExpanded] = useState(false);
  const [tab, setTab] = useState<Tab>("responses");
  const detailsRef = useRef<HTMLElement>(null);
  const forecastRef = useRef<HTMLDetailsElement>(null);
  const summary = data?.summary;
  const pageCount = data
    ? Math.max(1, Math.ceil(data.totalRows / request.pageSize))
    : 1;
  const page = data?.request.page ?? request.page;
  const firstRow = data?.rows.length ? page * request.pageSize + 1 : 0;
  const priced = summary?.pricedResponses ?? 0,
    unpriced = summary?.unpricedResponses ?? 0;
  const tabKeys = (event: KeyboardEvent<HTMLDivElement>) => {
    const current = tabs.findIndex(([key]) => key === tab);
    const next =
      event.key === "ArrowRight"
        ? (current + 1) % tabs.length
        : event.key === "ArrowLeft"
          ? (current + tabs.length - 1) % tabs.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? tabs.length - 1
              : null;
    if (next == null) return;
    event.preventDefault();
    setTab(tabs[next][0]);
    event.currentTarget
      .querySelectorAll<HTMLButtonElement>('[role="tab"]')
      [next]?.focus();
  };
  return (
    <div className="usage-dashboard" aria-busy={loading}>
      <div className="usage-dashboard-meta">
        <span className="usage-ledger-badge">
          <i />
          {isDemo ? "演示用量" : "本机响应账本"}
        </span>
        <span>
          {loading
            ? "正在更新…"
            : data
              ? `统计更新 ${usageTime(data.asOf, true)}`
              : "等待读取统计"}
        </span>
        <span>Asia/Shanghai · USD</span>
      </div>
      {error && (
        <div className="usage-error" role="alert">
          <strong>统计读取失败</strong>
          <span>{error}</span>
          {data && <small>保留上次统计，更新时间见上方。</small>}
        </div>
      )}
      <aside className={`usage-collection-notice${collectionCoverage.attention ? " needs-attention" : ""}`}
        aria-label="统计覆盖状态">
        <div>
          <strong role="status" aria-live="polite">{collectionCoverage.title}</strong>
          {collectionCoverage.notes.length > 0 && <p>{collectionCoverage.notes.join(" ")}</p>}
          <small>{isDemo ? "演示样本仅用于界面预览，不代表实际用量。"
            : "当前统计仅包含所选范围内已采集的本机记录，不代表账户或其它工具的全部用量。"}</small>
        </div>
        <button type="button" onClick={onOpenHealth}>查看来源与数据健康 →</button>
      </aside>
      <div className="usage-metrics">
        <section className="usage-metric">
          <span>
            估算总成本{" "}
            <span
              className="usage-info"
              title="按本地价格版本估算，仅统计已定价的有效响应，不代表账户实际扣费。"
            >
              ⓘ
            </span>
          </span>
          <strong
            className="usage-cost"
            title={
              summary?.estimatedCostUsd != null
                ? `精确估算 $${summary.estimatedCostUsd} USD`
                : undefined
            }
          >
            {summary ? summaryUsd(summary.estimatedCostUsd) : "—"}
          </strong>
          <small>
            {priced > 0
              ? `${usageNumber(priced)} 条已定价${unpriced ? ` · ${usageNumber(unpriced)} 条未定价` : " · 全部覆盖"}`
              : "配置价格后显示估算"}
          </small>
        </section>
        <section className="usage-metric">
          <span>
            有效计量响应{" "}
            <span
              className="usage-info"
              title="可信计量的有效响应记录数，与 HTTP 请求数和成功次数不同。冲突或计量无效的记录不进入可信统计。"
            >
              ⓘ
            </span>
          </span>
          <strong>
            {summary
              ? usageNumber(
                  summary.trustedMeteredResponseCount ??
                    Math.max(
                      0,
                      (summary.observedResponseCount ?? summary.responseCount) -
                        (summary.excludedResponses ?? 0),
                    ),
                )
              : "—"}
          </strong>
          <small>可信计量 · 非请求成功次数</small>
        </section>
        <section className="usage-metric">
          <span>
            总 Tokens{" "}
            <span
              className="usage-info"
              title="总量为输入加输出；缓存读取、缓存写入与推理输出属于细分，不重复相加。"
            >
              ⓘ
            </span>
          </span>
          <strong title={usageNumber(summary?.totalTokens)}>
            {summary ? usageNumber(summary.totalTokens, true) : "—"}
          </strong>
          <small>输入 + 输出 · 可信计量范围</small>
        </section>
        <section className="usage-metric">
          <span>
            缓存命中率{" "}
            <span
              className="usage-info"
              title="按 Σ缓存读取 / Σ有效输入 加权。缺失字段不按零计算，请结合字段覆盖率判断。"
            >
              ⓘ
            </span>
          </span>
          <strong>{summary ? percent(summary.cacheHitRate) : "—"}</strong>
          <small>
            输入量覆盖率 {summary ? percent(summary.cacheCoverage) : "—"}
          </small>
        </section>
      </div>
      <div className="usage-overview-actions">
        <button
          type="button"
          className="usage-more-toggle"
          aria-expanded={expanded}
          aria-controls="usage-extra-metrics"
          onClick={() => setExpanded(!expanded)}
        >
          {expanded ? "收起分项" : "分项指标"}
          <span aria-hidden="true">{expanded ? "⌃" : "⌄"}</span>
        </button>
        {data && (
          <nav className="usage-section-links" aria-label="统计区段">
            <button
              type="button"
              onClick={() => {
                detailsRef.current?.scrollIntoView({ block: "start" });
                detailsRef.current
                  ?.querySelector<HTMLButtonElement>('[aria-selected="true"]')
                  ?.focus({ preventScroll: true });
              }}
            >
              查看统计明细 <span aria-hidden="true">↓</span>
            </button>
            {tab !== "pricing" && (
              <button
                type="button"
                onClick={() => {
                  if (!forecastRef.current) return;
                  forecastRef.current.open = true;
                  forecastRef.current.scrollIntoView({ block: "start" });
                  forecastRef.current
                    .querySelector("summary")
                    ?.focus({ preventScroll: true });
                }}
              >
                Token 预测 <span aria-hidden="true">↓</span>
              </button>
            )}
          </nav>
        )}
      </div>
      {expanded && (
        <div className="usage-extra-metrics" id="usage-extra-metrics">
          {[
            [
              "输入总量",
              summary?.inputTokens,
              "来源记录的输入总量，包含缓存读取及适用的缓存写入细分",
            ],
            [
              "新增输入",
              summary?.normalInputTokens,
              "扣除缓存读取与有效缓存写入后的输入",
              responseCoverage(summary?.normalInputCoverage),
            ],
            ["输出", summary?.outputTokens, "包含推理输出细分，不重复计数"],
            [
              "推理输出",
              summary?.reasoningOutputTokens,
              "输出的细分，不重复相加；来源未提供时显示未提供",
            ],
            [
              "缓存写入",
              summary?.cacheWriteInputTokens,
              "来源未提供时显示未提供",
              responseCoverage(summary?.cacheWriteCoverage),
            ],
            [
              "缓存命中",
              summary?.cachedInputTokens,
              "来源记录的缓存读取 Token",
            ],
            [
              "已记录响应",
              summary?.observedResponseCount ?? summary?.responseCount,
              "当前范围内按响应标识去重的记录数，包含因冲突或无效计量而排除的记录；不等同于请求数或成功次数",
            ],
          ].map(([label, value, title, coverage]) => (
            <div key={String(label)} title={String(title)}>
              <span>{label}</span>
              <strong>
                {summary
                  ? usageNumber(value as number | null | undefined, true)
                  : "—"}
              </strong>
              {coverage && <small>{coverage}</small>}
            </div>
          ))}
        </div>
      )}
      {summary && ((summary.excludedResponses ?? 0) > 0 || unpriced > 0) && (
        <div className="usage-coverage-note">
          {unpriced > 0 && (
            <span>
              {usageNumber(unpriced)}{" "}
              条响应尚未定价，当前估算成本只包含已定价部分。
            </span>
          )}
          {(summary.excludedResponses ?? 0) > 0 && (
            <span>
              {usageNumber(summary.excludedResponses)}{" "}
              条冲突或计量无效响应已排除；原始记录仍保留在诊断中。
            </span>
          )}
        </div>
      )}
      {data && <UsageTrend data={data} />}
      {!data && (
        <div className="usage-loading" role="status">
          {loading
            ? "正在读取响应、趋势与价格版本…"
            : "统计暂不可用，请刷新后重试。"}
        </div>
      )}
      {data && (
        <section className="usage-details" ref={detailsRef}>
          <div className="usage-details-heading">
            <div
              className="usage-tabs"
              role="tablist"
              aria-label="用量统计视图"
              onKeyDown={tabKeys}
            >
              {tabs.map(([key, label]) => (
                <button
                  type="button"
                  id={`usage-tab-${key}`}
                  key={key}
                  role="tab"
                  aria-selected={tab === key}
                  aria-controls={`usage-panel-${key}`}
                  tabIndex={tab === key ? 0 : -1}
                  className={tab === key ? "active" : ""}
                  onClick={() => setTab(key)}
                >
                  {label}
                </button>
              ))}
            </div>
            {tab === "responses" && (
              <label className="usage-quality-filter">
                <span className="usage-sr-only">响应定价状态</span>
                <select
                  value={request.quality}
                  disabled={loading}
                  onChange={(event) =>
                    onRequestChange({
                      ...request,
                      page: 0,
                      quality: event.target
                        .value as UsageDashboardRequest["quality"],
                    })
                  }
                >
                  <option value="all">全部定价状态</option>
                  <option value="priced">已定价</option>
                  <option value="unpriced">未定价</option>
                </select>
              </label>
            )}
          </div>
          <div
            role="tabpanel"
            id={`usage-panel-${tab}`}
            aria-labelledby={`usage-tab-${tab}`}
            tabIndex={0}
          >
            {tab === "responses" && (
              <>
                <div className="usage-table-scroll">
                  <table className="usage-table usage-response-table">
                    <caption className="usage-sr-only">
                      响应明细。耗时来自同一回合，仅唯一有效响应回合显示首 Token
                      时间，未提供单响应生成速度。
                    </caption>
                    <thead>
                      <tr>
                        <th>时间</th>
                        <th>应用 / 供应商</th>
                        <th>模型</th>
                        <th className="number">新增输入</th>
                        <th className="number">输出</th>
                        <th className="number">缓存命中</th>
                        <th className="number">估算成本</th>
                        <th className="number">
                          速度{" "}
                          <span
                            className="usage-info"
                            title="来源没有可靠的单响应生成计时，暂不计算 Token/s。"
                          >
                            ⓘ
                          </span>
                        </th>
                        <th className="number">
                          回合首 Token{" "}
                          <span
                            className="usage-info"
                            title="Codex 记录的回合首 Token 耗时。仅唯一有效响应的回合提供可归因时间；多响应回合显示未提供。该值不是首可见文字延迟。"
                          >
                            ⓘ
                          </span>
                        </th>
                        <th>
                          <span className="usage-sr-only">打开会话</span>
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.rows.map((row) => (
                        <tr key={row.id}>
                          <td title={usageTime(row.eventTime, true)}>
                            {usageTime(row.eventTime)}
                            <small>{usageDay(row.eventTime)}</small>
                          </td>
                          <td>
                            <strong>{row.clientKind || "未知应用"}</strong>
                            <small>
                              {row.providerId && row.providerId !== "unknown"
                                ? row.providerId
                                : "供应商未知"}
                            </small>
                          </td>
                          <td
                            className="usage-model-cell"
                            title={`响应模型：${row.observedModel ?? "未知"}；请求模型：${row.requestedModel ?? "未知"}`}
                          >
                            <strong>
                              {row.observedModel ??
                                row.requestedModel ??
                                "未知"}
                            </strong>
                            {row.observedModel &&
                              row.requestedModel &&
                              row.observedModel !== row.requestedModel && (
                                <small>请求 {row.requestedModel}</small>
                              )}
                          </td>
                          <td
                            className="number"
                            title={usageNumber(row.normalInputTokens)}
                          >
                            {usageNumber(row.normalInputTokens, true)}
                          </td>
                          <td
                            className="number"
                            title={usageNumber(row.outputTokens)}
                          >
                            {usageNumber(row.outputTokens, true)}
                          </td>
                          <td
                            className="number"
                            title={usageNumber(row.cachedInputTokens)}
                          >
                            {usageNumber(row.cachedInputTokens, true)}
                          </td>
                          <td
                            className={`number ${row.estimatedCostUsd == null ? "usage-unavailable" : "usage-cost"}`}
                            title={
                              row.estimatedCostUsd == null
                                ? priceReasonLabel(row.priceReason)
                                : `价格版本 ${row.pricingVersionId ?? "未提供"} · 本地估算`
                            }
                          >
                            {usageUsd(row.estimatedCostUsd)}
                          </td>
                          <td className="number usage-unavailable">未提供</td>
                          <td
                            className="number"
                            title="回合首 Token 耗时；仅唯一响应回合可归因，非首可见文字延迟"
                          >
                            {duration(row.turnTimeToFirstTokenMs)}
                          </td>
                          <td>
                            <button
                              type="button"
                              className="usage-open-thread"
                              disabled={!row.threadId || !row.sourceId}
                              aria-label={
                                privacyMode
                                  ? "打开私密会话"
                                  : `打开会话 ${row.threadId}`
                              }
                              title={
                                privacyMode
                                  ? "打开私密会话"
                                  : `打开会话 ${row.threadId}`
                              }
                              onClick={() => {
                                if (row.threadId && row.sourceId)
                                  onOpenThread(row.threadId, row.sourceId);
                              }}
                            >
                              ↗
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                {!data.rows.length && (
                  <div className="usage-empty">
                    <strong>没有符合条件的响应</strong>
                    <span>
                      {request.quality === "all"
                        ? "尝试扩大日期范围或检查来源采集状态。"
                        : "可以切换定价状态，或到定价页补充模型价格。"}
                    </span>
                  </div>
                )}
                <div className="usage-pagination">
                  <span>
                    第 {firstRow}–
                    {data.rows.length ? firstRow + data.rows.length - 1 : 0}{" "}
                    条，共 {usageNumber(data.totalRows)} 条
                  </span>
                  <div>
                    <label>
                      每页{" "}
                      <select
                        aria-label="每页响应数"
                        value={request.pageSize}
                        disabled={loading}
                        onChange={(event) =>
                          onRequestChange({
                            ...request,
                            page: 0,
                            pageSize: Number(event.target.value),
                          })
                        }
                      >
                        {[20, 50, 100].map((size) => (
                          <option key={size} value={size}>
                            {size}
                          </option>
                        ))}
                      </select>
                    </label>
                    <button
                      type="button"
                      disabled={loading || page <= 0}
                      onClick={() =>
                        onRequestChange({ ...request, page: page - 1 })
                      }
                    >
                      上一页
                    </button>
                    <span>
                      {page + 1} / {pageCount}
                    </span>
                    <button
                      type="button"
                      disabled={loading || page + 1 >= pageCount}
                      onClick={() =>
                        onRequestChange({ ...request, page: page + 1 })
                      }
                    >
                      下一页
                    </button>
                  </div>
                </div>
                <p className="usage-table-note">
                  定价筛选仅影响明细。顶部指标、趋势和分组汇总覆盖完整筛选范围；回合首
                  Token 不能用于计算单响应速度。
                </p>
              </>
            )}
            {tab === "models" && (
              <AggregateTable rows={data.models} kind="模型" />
            )}
            {(tab === "projects" || tab === "sessions") && (
              <UsageScopeTable
                key={`${tab}:${JSON.stringify(query)}`}
                rows={tab === "projects" ? data.projects : data.sessions}
                kind={tab}
                query={query}
                onSelectScope={onSelectScope}
              />
            )}
            {tab === "providers" && (
              <AggregateTable rows={data.providers} kind="供应商" />
            )}
            {tab === "pricing" && (
              <UsagePricingPanel
                data={data}
                onSavePrice={onSavePrice}
                isDemo={isDemo}
                priceSyncConfig={priceSyncConfig}
                onSavePriceSyncConfig={onSavePriceSyncConfig}
                onSyncPricesNow={onSyncPricesNow}
                priceSyncing={priceSyncing}
              />
            )}
          </div>
        </section>
      )}
      {data && tab !== "pricing" && (
        <details className="usage-forecast-disclosure" ref={forecastRef}>
          <summary>
            <strong>Token 预测</strong>
            <span className="usage-forecast-disclosure-hint">
              未来 1 · 3 · 7 天与会话后续回合
            </span>
            <span className="usage-disclosure-closed">展开</span>
            <span className="usage-disclosure-open">收起</span>
          </summary>
          <UsageForecastPanel
            data={data}
            query={query}
            blockedReason={forecastBlockedReason}
          />
        </details>
      )}
    </div>
  );
}
