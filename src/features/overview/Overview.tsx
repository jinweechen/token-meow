import { useState } from "react";
import type { Snapshot } from "../../shared/contracts";
import {
  compact,
  dateTime,
  duration,
  formatNumber,
  latestTimedTurn,
  percent,
  statusLabel,
  isSuccess,
  accountExpired,
} from "../../shared/format";
import { Badge, Empty, Icon } from "../../shared/UI";
import { CatGirl } from "../pet/CatGirl";
import { projectSnapshotPresentation } from "../pet/petProjection";
import { OfficialUsageTrend } from "./OfficialUsageTrend";
import { sourceLabel } from "../../shared/filters";

export function Trend({ snapshot }: { snapshot: Snapshot }) {
  const [hover, setHover] = useState<number | null>(null);
  const points = snapshot.daily;
  if (!points.length)
    return (
      <Empty
        icon="grid"
        title="还没有可绘制的用量"
        detail="完整记录采集后，会在这里按上海时区显示每日用量。"
      />
    );
  const width = 680,
    height = 186,
    max = Math.max(1, ...points.map((point) => point.totalTokens));
  const coords = points.map((point, index) => ({
    x: 18 + (index / Math.max(1, points.length - 1)) * (width - 36),
    y: height - 18 - (point.totalTokens / max) * (height - 44),
  }));
  const path = coords
    .map((point, index) => `${index === 0 ? "M" : "L"}${point.x},${point.y}`)
    .join(" ");
  return (
    <div className="trend-chart">
      <div className="chart-y">
        <span>{compact(max)}</span>
        <span>{compact(max / 2)}</span>
        <span>0</span>
      </div>
      <div className="chart-drawing">
        <svg
          viewBox={`0 0 ${width} ${height}`}
          role="img"
          aria-label={`每日 token 用量曲线，共 ${points.length} 个记录日`}
        >
          <defs>
            <linearGradient id="chart-fill" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="#70c2ac" stopOpacity=".29" />
              <stop offset="100%" stopColor="#70c2ac" stopOpacity="0" />
            </linearGradient>
          </defs>
          {[32, 100, 168].map((y) => (
            <line
              key={y}
              x1="0"
              x2={width}
              y1={y}
              y2={y}
              stroke="#edf0eb"
              strokeDasharray="4 5"
            />
          ))}
          <path
            d={`${path} L${coords.at(-1)!.x},${height} L${coords[0].x},${height} Z`}
            fill="url(#chart-fill)"
          />
          <path
            d={path}
            fill="none"
            stroke="#47a68d"
            strokeWidth="3"
            strokeLinejoin="round"
            strokeLinecap="round"
          />
          {coords.map((point, index) => (
            <g key={points[index].date}>
              <circle
                cx={point.x}
                cy={point.y}
                r={hover === index ? 6 : 3.5}
                fill="#fff"
                stroke="#47a68d"
                strokeWidth="2"
              />
              <rect
                x={point.x - 15}
                y="0"
                width="30"
                height={height}
                fill="transparent"
                onMouseEnter={() => setHover(index)}
                onMouseLeave={() => setHover(null)}
              />
            </g>
          ))}
        </svg>
        <div className="chart-x">
          {points
            .filter(
              (_, index) =>
                points.length <= 8 ||
                index % Math.ceil(points.length / 7) === 0 ||
                index === points.length - 1,
            )
            .map((point) => (
              <span key={point.date}>
                {point.date.slice(5).replace("-", "/")}
              </span>
            ))}
        </div>
      </div>
      {hover != null && (
        <div className="chart-tooltip">
          {points[hover].date} · {formatNumber(points[hover].totalTokens)} token
        </div>
      )}
    </div>
  );
}

export function AccountCard({
  snapshot,
  onRefresh,
  pending,
}: {
  snapshot: Snapshot;
  onRefresh: () => void;
  pending: boolean;
}) {
  const account = snapshot.account;
  const expired = accountExpired(account);
  return (
    <section className="card account-card">
      <div className="section-heading">
        <h2>
          <Icon name="shield" size={18} /> 账户额度
        </h2>
        <button
          className="icon-button"
          title="刷新只读账户快照"
          aria-label="刷新账户额度"
          disabled={pending}
          onClick={onRefresh}
        >
          <Icon
            name="refresh"
            size={16}
            className={pending ? "spinning" : ""}
          />
        </button>
      </div>
      <div className="account-subtitle">
        <span>
          {sourceLabel(account?.sourceId, snapshot.sources)}
          {account?.sourceId === snapshot.primarySourceId && account?.sourceId
            ? " · Codex 主来源官方快照"
            : " · 账户快照"}{" "}
          · 与本机账本分别展示
        </span>
        <Badge
          tone={account?.status === "connected" && !expired ? "mint" : "amber"}
        >
          {expired
            ? "快照已过期"
            : account?.status === "connected"
              ? "已接入"
              : "暂不可用"}
        </Badge>
      </div>
      {expired && (
        <p className="subtle-note">
          下方保留历史值；等待实际重读，未自动确认额度恢复。
        </p>
      )}
      {account?.error && account.buckets.length > 0 && (
        <p className="subtle-note" role="status">
          最近读取提示：{account.error}
        </p>
      )}
      {account?.buckets.length ? (
        account.buckets.map((bucket) => (
          <div className="quota-bucket" key={bucket.id}>
            <strong>{bucket.name}</strong>
            {(
              [
                ["primary", bucket.primary],
                ["secondary", bucket.secondary],
              ] as const
            )
              .filter(([, window]) => window)
              .map(
                ([key, window]) =>
                  window && (
                    <div className="quota-window" key={key}>
                      <div>
                        <span>
                          {window.windowDurationMins == null
                            ? "来源未提供窗口"
                            : window.windowDurationMins >= 1440
                              ? `${Math.round(window.windowDurationMins / 1440)} 天窗口`
                              : `${window.windowDurationMins / 60} 小时窗口`}
                        </span>
                        <strong>
                          {window.usedPercent == null
                            ? "未提供"
                            : `${window.usedPercent}% 已用`}
                        </strong>
                      </div>
                      <div className="progress-track">
                        <span
                          style={{
                            width: `${Math.max(0, Math.min(100, window.usedPercent ?? 0))}%`,
                          }}
                        />
                      </div>
                      <div className="quota-meta">
                        <span>
                          {window.usedPercent == null
                            ? "剩余比例未提供"
                            : `剩余 ${Math.max(0, 100 - window.usedPercent).toFixed(0)}%`}
                        </span>
                        <span>重置 {dateTime(window.resetsAt)}</span>
                      </div>
                    </div>
                  ),
              )}
          </div>
        ))
      ) : (
        <div className="account-unavailable">
          <Icon name="shield" size={26} />
          <strong>等待账户快照</strong>
          <p>{account?.error ?? "账户接口尚未提供数据。额度不会被填成 0。"}</p>
        </div>
      )}
      <div className="card-footnote">
        <span className="status-dot" />
        采集于 {dateTime(account?.collectedAt)}
        {account?.lastAttemptAt && (
          <span> · 最近尝试 {dateTime(account.lastAttemptAt)}</span>
        )}
      </div>
      {account?.usage?.summary && (
        <div className="account-official">
          <span>官方累计 token</span>
          <strong>{compact(account.usage.summary.lifetimeTokens)}</strong>
          <small>官方口径，不与本机数据相加</small>
        </div>
      )}
      <OfficialUsageTrend account={account} />
    </section>
  );
}

export function Overview({
  snapshot,
  onPage,
  onChat,
  onAccountRefresh,
  pending,
}: {
  snapshot: Snapshot;
  onPage: (page: string) => void;
  onChat: (id: string, sourceId?: string) => void;
  onAccountRefresh: () => void;
  pending: boolean;
}) {
  const usage = snapshot.overview,
    timed = latestTimedTurn(snapshot.turns),
    privateMode = snapshot.settings.privacyMode;
  const stats = [
    {
      title: "总 token",
      value: compact(usage.totalTokens),
      icon: "sparkles" as const,
      detail: `${formatNumber(usage.responseCount)} 条已记录响应`,
      color: "mint",
    },
    {
      title: "输入 token",
      value: compact(usage.inputTokens),
      icon: "arrow" as const,
      detail: "包含缓存读取与有效缓存写入",
      color: "blue",
    },
    {
      title: "输出 token",
      value: compact(usage.outputTokens),
      icon: "terminal" as const,
      detail: `推理输出 ${compact(usage.reasoningOutputTokens)}`,
      color: "peach",
    },
    {
      title: "缓存命中率",
      value: percent(usage.cacheHitRate),
      icon: "git" as const,
      detail: `输入量覆盖率 ${percent(usage.cacheCoverage)}`,
      color: "lavender",
    },
  ];
  return (
    <>
      <section className="welcome-banner">
        <div className="welcome-copy">
          <div className="eyebrow">
            <span className="tiny-sparkle">✦</span> LOCAL FIRST, ALWAYS.
          </div>
          <h2>
            灵感在前进，
            <br />
            小柚在身边。
          </h2>
          <p>
            把每一次用量记清楚，
            <br className="mobile-break" />
            让创造更从容一点。
          </p>
          <div className="welcome-pills">
            <span>
              <span className="status-dot" />
              仅本机只读采集
            </span>
            <span>
              <Icon name="volume" size={13} />
              默认安静陪伴
            </span>
          </div>
        </div>
        <div className="welcome-pet">
          <div className="pet-orbit orbit-one" />
          <div className="pet-orbit orbit-two" />
          <span className="floating-star star-one">✧</span>
          <span className="floating-star star-two">+</span>
          <CatGirl presentation={projectSnapshotPresentation(snapshot)} ready decorative />
          <span className="pet-name-tag">
            小柚 <span>NEKOMI</span>
          </span>
        </div>
        <span className="banner-pattern">✦</span>
      </section>
      <div className="stat-grid">
        {stats.map((stat) => (
          <section className="card stat-card" key={stat.title}>
            <div className="stat-label">
              {stat.title}
              <span className={`stat-icon ${stat.color}`}>
                <Icon name={stat.icon} size={17} />
              </span>
            </div>
            <strong
              className="stat-value"
              title={
                stat.title === "总 token"
                  ? formatNumber(usage.totalTokens)
                  : undefined
              }
            >
              {stat.value}
            </strong>
            <span className="stat-detail">{stat.detail}</span>
          </section>
        ))}
      </div>
      <div className="overview-grid">
        <div className="overview-main">
          <section className="card trend-card">
            <div className="section-heading">
              <div>
                <h2>用量走势</h2>
                <p>本机有效响应 · Asia/Shanghai</p>
              </div>
              <span className="legend">
                <i />总 token
              </span>
            </div>
            <Trend snapshot={snapshot} />
            <div className="chart-footer">
              <Icon name="info" size={14} />
              <span>总量 = 输入 + 输出；缓存与推理是细分，不重复累加。</span>
            </div>
          </section>
          <section className="card projects-card">
            <div className="section-heading">
              <h2>灵感落在这些项目</h2>
              <button
                className="text-button"
                onClick={() => onPage("sessions")}
              >
                查看全部 <Icon name="arrow" size={15} />
              </button>
            </div>
            {snapshot.projects.length ? (
              <div className="project-list">
                {snapshot.projects.slice(0, 4).map((project, index) => (
                  <div className="project-row" key={project.id}>
                    <span className={`project-avatar color-${index % 3}`}>
                      <Icon name="folder" size={20} />
                    </span>
                    <div className="project-description">
                      <strong>
                        {privateMode ? `项目 ${index + 1}` : project.name}
                      </strong>
                      <span>{project.responseCount} 条响应</span>
                    </div>
                    <div className="project-usage">
                      <strong>{compact(project.totalTokens)}</strong>
                      <div className="progress-track">
                        <span
                          style={{
                            width: `${usage.totalTokens ? (project.totalTokens / usage.totalTokens) * 100 : 0}%`,
                          }}
                        />
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            ) : (
              <Empty
                icon="folder"
                title="项目列表会在采集后出现"
                detail="保留原始目录，也支持把多个 worktree 归入同一项目。"
              />
            )}
          </section>
        </div>
        <div className="overview-aside">
          <section className="card cache-card">
            <div className="section-heading">
              <h2>缓存小账本</h2>
              <span className="circle-icon">
                <Icon name="git" size={16} />
              </span>
            </div>
            <div className="cache-hero">
              <strong>{compact(usage.cachedInputTokens)}</strong>
              <span>已记录缓存读取 token</span>
            </div>
            <dl className="metric-list">
              <div>
                <dt>普通输入</dt>
                <dd>{compact(usage.normalInputTokens)}</dd>
              </div>
              <div>
                <dt>缓存写入</dt>
                <dd>{compact(usage.cacheWriteInputTokens)}</dd>
              </div>
              <div>
                <dt>推理输出</dt>
                <dd>{compact(usage.reasoningOutputTokens)}</dd>
              </div>
            </dl>
            <p className="subtle-note">
              命中率按 Σ缓存读取 / Σ有效输入 加权；缺失字段不按未命中计算。
            </p>
          </section>
          <AccountCard
            snapshot={snapshot}
            onRefresh={onAccountRefresh}
            pending={pending}
          />
        </div>
      </div>
      <div className="bottom-grid">
        <section className="card timing-card">
          <div className="section-heading">
            <h2>
              <Icon name="clock" size={18} /> 最近回合时间
            </h2>
            <Badge>来源记录</Badge>
          </div>
          <div className="timing-values">
            <div>
              <strong>{duration(timed?.timeToFirstTokenMs)}</strong>
              <span>Codex 记录的回合首 token 耗时</span>
            </div>
            <div>
              <strong>{duration(timed?.durationMs)}</strong>
              <span>该回合总耗时</span>
            </div>
          </div>
          <p className="subtle-note">
            此指标不等于首可见文字延迟，暂不提供 token/s。
          </p>
        </section>
        <section className="card recent-card">
          <div className="section-heading">
            <h2>最近的回音</h2>
            <button className="text-button" onClick={() => onPage("inbox")}>
              收件箱 <Icon name="arrow" size={15} />
            </button>
          </div>
          {snapshot.inbox.length ? (
            snapshot.inbox.slice(0, 2).map((event) => (
              <button
                className="recent-event"
                key={event.id}
                onClick={() => onChat(event.threadId, event.sourceId)}
              >
                <span
                  className={`event-icon ${isSuccess(event.status) ? "success" : "warning"}`}
                >
                  <Icon
                    name={isSuccess(event.status) ? "check" : "info"}
                    size={16}
                  />
                </span>
                <span>
                  <strong>{privateMode ? "私密会话" : event.title}</strong>
                  <small>
                    {statusLabel(event.status)} ·{" "}
                    {dateTime(event.createdAt, true)}
                  </small>
                </span>
                <Icon name="chevron" size={15} />
              </button>
            ))
          ) : (
            <p className="quiet-empty">
              暂时没有回合结束记录。小柚会安静等待。
            </p>
          )}
        </section>
      </div>
      {(usage.excludedResponses ?? 0) > 0 && (
        <div className="notice">
          <Icon name="shield" size={17} />
          <span>
            {usage.excludedResponses}{" "}
            条冲突或计量无效的响应已从可信总量排除。进入可信总量{" "}
            {usage.trustedMeteredResponseCount ?? "未提供"} 条；已记录响应身份共{" "}
            {usage.responseCount} 条。
          </span>
        </div>
      )}
      {usage.legacySegments > 0 && (
        <div className="notice">
          <Icon name="info" size={17} />
          <span>
            另外记录 {usage.legacySegments} 个旧格式推导片段、
            {compact(usage.legacyTokens)}{" "}
            token。它们属于受限估算，未与有效响应账本混加。
          </span>
        </div>
      )}
    </>
  );
}
