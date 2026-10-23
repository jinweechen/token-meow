import { useState } from "react";
import type { Query, Snapshot } from "../../shared/contracts";
import type { DatePeriod, QueryUpdate } from "../../shared/queryLifecycle";
import {
  applyAgentKindFilter,
  applyRootFilter,
  applySessionFilter,
  applySourceFilter,
  identityValue,
  rootTurnOptions,
  sessionFilterOptions,
  sessionFilterLabel,
  sourceLabel,
} from "../../shared/filters";
import {
  activeScopeFilterKeys,
  clearScopeFilters,
  parentScopeQuery,
  removeScopeFilter,
  type ScopeFilterKey,
} from "../../shared/queryPresentation";
import { Icon } from "../../shared/UI";
import { sourceKindName } from "../../shared/sourceKind";
import "./queryFilters.css";

interface Props {
  query: Query;
  snapshot: Snapshot;
  period: DatePeriod;
  page: string;
  onQueryChange: (next: QueryUpdate) => void;
  onPeriodChange: (period: DatePeriod) => void;
  onRefresh: () => void;
}

export function QueryFilters({
  query,
  snapshot,
  period,
  page,
  onQueryChange,
  onPeriodChange,
  onRefresh,
}: Props) {
  const [advanced, setAdvanced] = useState(false);
  const privateMode = snapshot.settings.privacyMode;
  const sessions = sessionFilterOptions(snapshot.sessions, query);
  const roots = rootTurnOptions(snapshot.turns, snapshot.sessions);
  const selectedSession =
    query.threadId && query.sourceId
      ? { id: query.threadId, sourceId: query.sourceId }
      : null;
  const selectedRoot =
    query.rootTurnId && query.sourceId
      ? { id: query.rootTurnId, sourceId: query.sourceId }
      : null;
  if (
    selectedRoot &&
    !roots.some((root) => identityValue(root) === identityValue(selectedRoot))
  )
    roots.push(selectedRoot);
  const active = activeScopeFilterKeys(query);
  const advancedKeys = active.filter(
    (key) => !["project", "model"].includes(key),
  );
  const parent = parentScopeQuery(query);
  const setField = (key: ScopeFilterKey, value: string) =>
    onQueryChange((previous) => ({ ...previous, [key]: value || undefined }));
  const filterLabel = (key: ScopeFilterKey): string => {
    switch (key) {
      case "project": {
        const index = snapshot.projects.findIndex(
          (project) => project.id === query.project,
        );
        return `项目：${privateMode ? "已隐藏" : snapshot.projects[index]?.name || "已选项目"}`;
      }
      case "threadId": {
        const index = sessions.findIndex(
          (session) =>
            session.id === query.threadId &&
            session.sourceId === query.sourceId,
        );
        const title = privateMode
          ? "已隐藏"
          : sessions[index]?.title || "已选会话";
        return `会话：${title}`;
      }
      case "rootTurnId":
        return `根回合：${privateMode ? "已隐藏" : query.rootTurnId?.slice(0, 12)}`;
      case "sourceId":
        return `来源：${sourceLabel(query.sourceId, snapshot.sources)}`;
      case "model":
        return `模型：${query.model}`;
      case "clientKind":
        return `工具：${sourceKindName(query.clientKind)}`;
      case "providerId":
        return `提供商：${query.providerId === "unknown" ? "未知" : query.providerId}`;
      case "agentKind":
        return `代理：${({ primary: "主代理", subagent: "子代理", unknown: "归属未知" } as const)[query.agentKind!]}`;
    }
  };
  return (
    <section className="query-panel usage-query" aria-label="用量筛选">
      <div className="usage-query-top">
        <div className="period-tabs" role="group" aria-label="日期范围">
          {(
            [
              ["1", "今日"],
              ["7", "近 7 天"],
              ["30", "近 30 天"],
              ["custom", "自定义"],
            ] as const
          ).map(([id, label]) => (
            <button
              type="button"
              key={id}
              aria-pressed={period === id}
              className={period === id ? "active" : ""}
              onClick={() => onPeriodChange(id)}
            >
              {label}
            </button>
          ))}
        </div>
        <div className="usage-query-common">
          <label>
            <span className="usage-sr-only">项目筛选</span>
            <select
              aria-label="项目筛选"
              value={query.project ?? ""}
              onChange={(event) => setField("project", event.target.value)}
            >
              <option value="">全部项目</option>
              {query.project &&
                !snapshot.projects.some(
                  (project) => project.id === query.project,
                ) && <option value={query.project}>已选项目</option>}
              {snapshot.projects.map((project, index) => (
                <option key={project.id} value={project.id}>
                  {privateMode ? `项目 ${index + 1}` : project.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span className="usage-sr-only">模型筛选</span>
            <select
              aria-label="模型筛选"
              value={query.model ?? ""}
              onChange={(event) => setField("model", event.target.value)}
            >
              <option value="">全部模型</option>
              {query.model &&
                !snapshot.models.some(
                  (model) => model.name === query.model,
                ) && <option value={query.model}>{query.model}</option>}
              {snapshot.models.map((model) => (
                <option key={model.name} value={model.name}>
                  {model.name || "未知模型"}
                </option>
              ))}
            </select>
          </label>
        </div>
        <button
          type="button"
          className={`usage-query-more ${advanced ? "active" : ""}`}
          aria-expanded={advanced}
          aria-controls="usage-advanced-filters"
          onClick={() => setAdvanced(!advanced)}
        >
          <Icon name="filter" size={14} />
          更多筛选
          {advancedKeys.length > 0 && <span>{advancedKeys.length}</span>}
          <Icon name="chevron" size={12} />
        </button>
        <button
          type="button"
          className="icon-button usage-query-refresh"
          aria-label="刷新用量视图"
          title="刷新用量视图"
          onClick={onRefresh}
        >
          <Icon name="refresh" size={17} />
        </button>
      </div>
      {period === "custom" && (
        <div className="usage-query-dates">
          <label>
            开始日期
            <input
              type="date"
              aria-label="开始日期"
              value={query.from ?? ""}
              max={query.to}
              onChange={(event) =>
                onQueryChange((previous) => ({
                  ...previous,
                  from: event.target.value,
                }))
              }
            />
          </label>
          <span>至</span>
          <label>
            结束日期
            <input
              type="date"
              aria-label="结束日期"
              value={query.to ?? ""}
              min={query.from}
              onChange={(event) =>
                onQueryChange((previous) => ({
                  ...previous,
                  to: event.target.value,
                }))
              }
            />
          </label>
        </div>
      )}
      {advanced && (
        <div id="usage-advanced-filters" className="usage-query-advanced">
          <label>
            采集来源
            <select
              aria-label="来源筛选"
              value={query.sourceId ?? ""}
              onChange={(event) =>
                onQueryChange((previous) =>
                  applySourceFilter(previous, event.target.value),
                )
              }
            >
              <option value="">全部已接入来源</option>
              {query.sourceId &&
                !snapshot.sources?.some(
                  (source) => source.id === query.sourceId,
                ) && <option value={query.sourceId}>已选来源</option>}
              {snapshot.sources?.map((source) => (
                <option key={source.id} value={source.id}>
                  {sourceLabel(source.id, snapshot.sources)}
                </option>
              ))}
            </select>
          </label>
          <label>
            会话
            <select
              aria-label="会话筛选"
              title="切换会话会清除其它明细条件并保留日期；统计表下钻则保留当前交集。"
              value={selectedSession ? identityValue(selectedSession) : ""}
              onChange={(event) =>
                onQueryChange((previous) =>
                  applySessionFilter(previous, event.target.value),
                )
              }
            >
              <option value="">全部会话</option>
              {sessions.map((session, index) => (
                <option
                  key={identityValue(session)}
                  value={identityValue(session)}
                >
                  {sessionFilterLabel(
                    session,
                    index,
                    !!privateMode,
                    snapshot.sources,
                  )}
                </option>
              ))}
            </select>
          </label>
          {page !== "sessions" && (
            <label>
              代理类型
              <select
                aria-label="代理类型筛选"
                value={query.agentKind ?? ""}
                onChange={(event) =>
                  onQueryChange((previous) =>
                    applyAgentKindFilter(previous, event.target.value),
                  )
                }
              >
                <option value="">全部代理类型</option>
                <option value="primary">主代理</option>
                <option value="subagent">子代理</option>
                <option value="unknown">归属未知</option>
              </select>
            </label>
          )}
          <label>
            AI 工具
            <select
              aria-label="AI 工具筛选"
              value={query.clientKind ?? ""}
              onChange={(event) => setField("clientKind", event.target.value)}
            >
              <option value="">全部已接入工具</option>
              {query.clientKind &&
                !(snapshot.filters?.clientKinds ?? ["codex"]).includes(
                  query.clientKind,
                ) && (
                  <option value={query.clientKind}>
                    {sourceKindName(query.clientKind)}
                  </option>
                )}
              {(snapshot.filters?.clientKinds ?? ["codex"]).map((kind) => (
                <option key={kind} value={kind}>
                  {sourceKindName(kind)}
                </option>
              ))}
            </select>
          </label>
          <label>
            提供商
            <select
              aria-label="提供商筛选"
              value={query.providerId ?? ""}
              onChange={(event) => setField("providerId", event.target.value)}
            >
              <option value="">全部提供商</option>
              {query.providerId &&
                !(snapshot.filters?.providerIds ?? ["unknown"]).includes(
                  query.providerId,
                ) && (
                  <option value={query.providerId}>
                    {query.providerId === "unknown"
                      ? "未知提供商"
                      : query.providerId}
                  </option>
                )}
              {(snapshot.filters?.providerIds ?? ["unknown"]).map(
                (provider) => (
                  <option key={provider} value={provider}>
                    {provider === "unknown" ? "未知提供商" : provider}
                  </option>
                ),
              )}
            </select>
          </label>
          {page === "sessions" && (
            <label>
              根回合
              <select
                aria-label="根回合筛选"
                value={selectedRoot ? identityValue(selectedRoot) : ""}
                onChange={(event) =>
                  onQueryChange((previous) =>
                    applyRootFilter(previous, event.target.value),
                  )
                }
              >
                <option value="">全部根回合</option>
                {roots.map((turn, index) => (
                  <option key={identityValue(turn)} value={identityValue(turn)}>
                    {privateMode
                      ? `根回合 ${index + 1}`
                      : `${turn.id.slice(0, 12)} · ${sourceLabel(turn.sourceId, snapshot.sources)}`}
                  </option>
                ))}
              </select>
            </label>
          )}
        </div>
      )}
      {active.length > 0 && (
        <div className="usage-query-selection" aria-label="已选筛选条件">
          <span className="usage-query-selection-label">当前范围</span>
          <div className="usage-query-chips">
            {active.map((key) => (
              <button
                type="button"
                key={key}
                className="usage-query-chip"
                aria-label={`移除${filterLabel(key)}`}
                title={`移除${filterLabel(key)}`}
                onClick={() =>
                  onQueryChange((previous) => removeScopeFilter(previous, key))
                }
              >
                <span>{filterLabel(key)}</span>
                <Icon name="close" size={12} />
              </button>
            ))}
          </div>
          <div className="usage-query-selection-actions">
            {parent && (
              <button
                type="button"
                onClick={() =>
                  onQueryChange(
                    (previous) => parentScopeQuery(previous) ?? previous,
                  )
                }
              >
                返回上一级
              </button>
            )}
            <button
              type="button"
              onClick={() =>
                onQueryChange((previous) => clearScopeFilters(previous))
              }
            >
              清除筛选
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
