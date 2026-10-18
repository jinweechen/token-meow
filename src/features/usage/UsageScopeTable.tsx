import { useState } from "react";
import type { Query, UsageDashboardScopeGroup } from "../../shared/contracts";
import { percent } from "../../shared/format";
import { usageNumber, usageTime, usageUsd } from "../../shared/usageDashboard";
import { usageScopeQuery } from "../../shared/usageScope";
import "./usageScopes.css";

export function UsageScopeTable({
  rows,
  kind,
  query,
  onSelectScope,
}: {
  rows: UsageDashboardScopeGroup[];
  kind: "projects" | "sessions";
  query: Query;
  onSelectScope: (next: Query) => void;
}) {
  const [pageIndex, setPageIndex] = useState(0);
  const [showBreakdown, setShowBreakdown] = useState(false);
  const pageSize = 20;
  const pageCount = Math.max(1, Math.ceil(rows.length / pageSize));
  const page = Math.min(pageIndex, pageCount - 1);
  const label = kind === "projects" ? "项目" : "会话";
  return (
    <>
      <div className="usage-aggregate-heading">
        <span>
          {label}用量汇总 · {usageNumber(rows.length)} 个{label}
        </span>
        <span>按总 Tokens 降序</span>
      </div>
      <label className="usage-scope-breakdown">
        <input
          type="checkbox"
          checked={showBreakdown}
          onChange={(event) => setShowBreakdown(event.target.checked)}
        />
        显示细分列
        <span>响应、输入输出、缓存和活动时间</span>
      </label>
      <div className="usage-table-scroll usage-scope-scroll">
        <table
          className={`usage-table usage-scope-table${showBreakdown ? " show-breakdown" : ""}`}
        >
          <caption className="usage-sr-only">
            当前筛选范围内的{label} Token 用量，包含缓存输入
          </caption>
          <thead>
            <tr>
              <th className="usage-scope-name">{label}</th>
              {kind === "projects" && (
                <th className="number usage-scope-detail">会话数</th>
              )}
              <th className="number usage-scope-detail">有效响应</th>
              <th className="number usage-scope-detail">输入</th>
              <th className="number usage-scope-detail">输出</th>
              <th className="number usage-scope-detail">缓存读取</th>
              <th className="number" aria-sort="descending">
                总 Tokens ↓
              </th>
              <th className="number">估算成本</th>
              <th className="usage-scope-detail">最近活动</th>
              <th>
                <span className="usage-sr-only">查看该范围</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.slice(page * pageSize, (page + 1) * pageSize).map((row) => {
              const next = usageScopeQuery(query, kind, row);
              return (
                <tr key={row.key}>
                  <td className="usage-scope-name">
                    <strong title={row.name}>{row.name || "归属未知"}</strong>
                  </td>
                  {kind === "projects" && (
                    <td className="number usage-scope-detail">
                      {usageNumber(row.sessionCount)}
                    </td>
                  )}
                  <td className="number usage-scope-detail">
                    {usageNumber(row.responseCount)}
                  </td>
                  <td
                    className="number usage-scope-detail"
                    title={usageNumber(row.inputTokens)}
                  >
                    {usageNumber(row.inputTokens, true)}
                  </td>
                  <td
                    className="number usage-scope-detail"
                    title={usageNumber(row.outputTokens)}
                  >
                    {usageNumber(row.outputTokens, true)}
                  </td>
                  <td
                    className="number usage-scope-detail"
                    title={`缓存读取 ${usageNumber(row.cachedInputTokens)} Tokens；缓存字段覆盖 ${percent(row.cacheCoverage)}`}
                  >
                    {usageNumber(row.cachedInputTokens, true)}
                    <small>{percent(row.cacheHitRate)} 命中</small>
                  </td>
                  <td className="number" title={usageNumber(row.totalTokens)}>
                    <strong>{usageNumber(row.totalTokens, true)}</strong>
                  </td>
                  <td
                    className={`number ${row.estimatedCostUsd == null ? "usage-unavailable" : "usage-cost"}`}
                    title={
                      row.estimatedCostUsd == null
                        ? "缺少有效价格"
                        : `USD ${row.estimatedCostUsd}`
                    }
                  >
                    {usageUsd(row.estimatedCostUsd)}
                    <small>
                      已定价 {row.pricedResponses} / {row.responseCount}
                    </small>
                  </td>
                  <td className="usage-scope-detail">
                    {usageTime(row.lastEventTime, true)}
                  </td>
                  <td>
                    <button
                      type="button"
                      className="usage-scope-open"
                      disabled={!next}
                      title={
                        next
                          ? `查看${label}的统计与预测`
                          : "隐私模式或缺少来源身份，暂不支持定位"
                      }
                      onClick={() => {
                        if (next) onSelectScope(next);
                      }}
                    >
                      统计与预测 <span aria-hidden="true">↗</span>
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {!rows.length && (
        <div className="usage-empty">
          <strong>暂无{label}统计</strong>
          <span>扩大日期范围或检查来源采集状态后重试。</span>
        </div>
      )}
      {rows.length > pageSize && (
        <div className="usage-pagination">
          <span>
            共 {usageNumber(rows.length)} 个{label}
          </span>
          <div>
            <button
              type="button"
              disabled={page === 0}
              onClick={() => setPageIndex(page - 1)}
            >
              上一页
            </button>
            <span>
              {page + 1} / {pageCount}
            </span>
            <button
              type="button"
              disabled={page + 1 >= pageCount}
              onClick={() => setPageIndex(page + 1)}
            >
              下一页
            </button>
          </div>
        </div>
      )}
      <p className="usage-table-note">
        输入包含缓存读取与缓存写入；总量为输入加输出。点击后保留日期、模型等当前筛选，仅收窄到所选
        {label}。
      </p>
    </>
  );
}
