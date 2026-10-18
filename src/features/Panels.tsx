import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "../shared/bridge";
import type {
  AgentKind,
  Budget,
  Session,
  Settings,
  Snapshot,
} from "../shared/contracts";
import {
  compact,
  dateTime,
  duration,
  formatNumber,
  isSuccess,
  issueCount,
  statusLabel,
} from "../shared/format";
import { Badge, Empty, Icon, Modal, Toggle } from "../shared/UI";
import { builtinAnimationManifest as builtinManifest } from "./pet/builtinAnimationSkin";
import { AccountCard } from "./overview/Overview";
import { SourceDetails } from "./sources/SourceDetails";
import { CompanionSettings } from "./companion/DesktopCompanion";
import { LocalPetSkinSettings } from "./pet/LocalPetSkinSettings";
import { useLocalPetSkins } from "./pet/useLocalPetSkins";
import { localPetSkinClient } from "./pet/localPetSkinClient";
import { TemporaryInteractionSettings } from "./pet/TemporaryInteractionSettings";
import { usePetExtensionCapabilities } from "./pet/usePetExtensionCapabilities";
import {
  agentKindLabel,
  identityValue,
  parseIdentity,
  projectLabel,
  turnsForSession,
  sourceLabel,
  rootTurnOptions,
  sessionAgentKind,
} from "../shared/filters";

type Action = (
  name: string,
  payload?: Record<string, unknown>,
) => Promise<void>;
export function SessionsPanel({
  snapshot,
  onChat,
  onUsage,
  action,
  agentKind,
  onAgentKindChange,
}: {
  snapshot: Snapshot;
  onChat: (id: string, sourceId?: string) => void;
  onUsage?: (id: string, sourceId: string) => void;
  action: Action;
  agentKind?: AgentKind;
  onAgentKindChange: (value: string) => void;
}) {
  const [search, setSearch] = useState(""),
    [selection, setSelection] = useState<Pick<Session, "sourceId" | "id"> | null>(null),
    [mapping, setMapping] = useState("");
  const selected = selection
    ? snapshot.sessions.find(
        (session) => session.id === selection.id && session.sourceId === selection.sourceId,
      )
    : undefined;
  useEffect(() => {
    if (selection && !selected) setSelection(null);
  }, [selection, selected]);
  const privateMode = !!snapshot.settings.privacyMode;
  const sessions = snapshot.sessions.filter(
    (session) =>
      privateMode ||
      `${session.title} ${session.cwd} ${session.model}`
        .toLowerCase()
        .includes(search.toLowerCase()),
  );
  return (
    <>
      <div className="panel-toolbar">
        <div className="tabs">
          {[
            ["all", "全部会话"],
            ["primary", "主代理"],
            ["subagent", "子代理"],
            ["unknown", "归属未知"],
          ].map(([key, label]) => (
            <button
              key={key}
              className={(agentKind ?? "all") === key ? "active" : ""}
              onClick={() => onAgentKindChange(key === "all" ? "" : key)}
            >
              {label}
              {key === "all" && <span>{snapshot.sessions.length}</span>}
            </button>
          ))}
        </div>
        <label className="search-box">
          <Icon name="search" size={17} />
          <input
            aria-label="搜索会话"
            placeholder={
              privateMode ? "隐私模式下不搜索标题" : "搜索会话、目录或模型…"
            }
            value={search}
            disabled={privateMode}
            onChange={(e) => setSearch(e.target.value)}
          />
        </label>
      </div>
      <section className="card table-card">
        {sessions.length ? (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>会话 / 项目</th>
                  <th>记录的请求模型</th>
                  <th>代理关系</th>
                  <th>总 token</th>
                  <th>响应</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {sessions.map((session, index) => (
                  <tr key={`${session.id}-${index}`}>
                    <td>
                      <button
                        className="table-title"
                        onClick={() => {
                          setSelection({ sourceId: session.sourceId, id: session.id });
                          setMapping(projectLabel(session, snapshot.projects));
                        }}
                      >
                        <Icon
                          name={
                            sessionAgentKind(session) === "subagent"
                              ? "git"
                              : "folder"
                          }
                          size={18}
                        />
                        <span>
                          <strong>
                            {privateMode
                              ? `会话 ${index + 1}`
                              : session.title ||
                                `会话 ${session.id.slice(0, 8)}`}
                          </strong>
                          <small>
                            {privateMode
                              ? "项目和目录已隐藏"
                              : `${projectLabel(session, snapshot.projects)} · ${sourceLabel(session.sourceId, snapshot.sources)}`}
                          </small>
                        </span>
                      </button>
                    </td>
                    <td>
                      <code className="model-label">
                        {session.model || "未知模型"}
                      </code>
                    </td>
                    <td>
                      <Badge
                        tone={
                          sessionAgentKind(session) === "subagent"
                            ? "lavender"
                            : sessionAgentKind(session) === "primary"
                              ? "mint"
                              : "neutral"
                        }
                      >
                        {agentKindLabel(sessionAgentKind(session))}
                      </Badge>
                    </td>
                    <td className="numeric">
                      {session.totalTokens == null
                        ? "未提供"
                        : compact(session.totalTokens)}
                    </td>
                    <td>{formatNumber(session.responseCount)}</td>
                    <td>
                      {onUsage && (
                        <button
                          className="icon-button"
                          aria-label="查看用量与预测"
                          title="查看用量与预测"
                          disabled={privateMode || !session.sourceId}
                          onClick={() => {
                            if (session.sourceId)
                              onUsage(session.id, session.sourceId);
                          }}
                        >
                          <Icon name="grid" size={17} />
                        </button>
                      )}
                      <button
                        className="icon-button"
                        aria-label={
                          session.sourceId ? "打开聊天" : "复制会话标识"
                        }
                        onClick={() => onChat(session.id, session.sourceId)}
                      >
                        <Icon name="external" size={17} />
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <Empty
            icon="folder"
            title={search ? "没有匹配的会话" : "尚未记录会话"}
            detail={
              search
                ? "试试其他关键词或切换代理类型。"
                : "从已验证的日志事实读取会话身份，未知模型不会用当前配置补填。"
            }
          />
        )}
      </section>
      <div className="notice">
        <Icon name="git" size={18} />
        <span>
          代理类型依据来源记录；缺证、版本未验证或关系冲突显示归属未知。子代理保留自己的用量，父会话缺失时不推断根任务。
        </span>
      </div>
      {selected && (
        <Modal title="会话明细" onClose={() => setSelection(null)}>
          <div className="session-detail">
            <h3>{privateMode ? "私密会话" : selected.title}</h3>
            <p className="mono">
              {privateMode ? "会话标识已隐藏" : selected.id}
            </p>
            <dl className="metric-list">
              <div>
                <dt>请求模型</dt>
                <dd>{selected.model || "未知模型"}</dd>
              </div>
              <div>
                <dt>总 token / 已记录响应</dt>
                <dd>
                  {selected.totalTokens == null
                    ? "未提供"
                    : compact(selected.totalTokens)}{" "}
                  / {selected.responseCount}
                </dd>
              </div>
              <div>
                <dt>父会话</dt>
                <dd>
                  {selected.parentThreadId
                    ? privateMode
                      ? "已隐藏"
                      : selected.parentThreadId.slice(0, 12)
                    : "未记录父会话"}
                </dd>
              </div>
              <div>
                <dt>原始目录</dt>
                <dd className="wrap-path">
                  {privateMode ? "已隐藏" : selected.cwd || "未记录"}
                </dd>
              </div>
            </dl>
            <h4>来源记录的回合</h4>
            {turnsForSession(snapshot.turns, selected).length ? (
              turnsForSession(snapshot.turns, selected).map((turn) => (
                <div className="detail-turn" key={turn.id}>
                  <Badge tone={isSuccess(turn.status) ? "mint" : "amber"}>
                    {statusLabel(turn.status)}
                  </Badge>
                  <span>
                    总耗时 {duration(turn.durationMs)}
                    <br />
                    <small>
                      回合首 token 耗时 {duration(turn.timeToFirstTokenMs)}
                    </small>
                  </span>
                </div>
              ))
            ) : (
              <p className="subtle-note">此会话没有已记录的回合时间。</p>
            )}
            {!privateMode && selected.cwd && (
              <form
                className="mapping-form"
                onSubmit={(event) => {
                  event.preventDefault();
                  void action("set_project_mapping", {
                    path: selected.cwd,
                    project: mapping,
                  });
                }}
              >
                <label>
                  把此目录归入项目
                  <input
                    required
                    value={mapping}
                    onChange={(event) => setMapping(event.target.value)}
                    placeholder="项目显示名称"
                    maxLength={120}
                  />
                </label>
                <button className="button secondary" type="submit">
                  保存项目映射
                </button>
              </form>
            )}
            <div className="modal-actions">
              <button
                className="button secondary"
                onClick={() => {
                  void navigator.clipboard.writeText(selected.id);
                }}
              >
                <Icon name="copy" size={16} />
                复制会话标识
              </button>
              <button
                className="button primary"
                onClick={() => onChat(selected.id, selected.sourceId)}
              >
                返回聊天
                <Icon name="external" size={16} />
              </button>
            </div>
          </div>
        </Modal>
      )}
    </>
  );
}

export function ToolsPanel({
  snapshot,
  onChat,
}: {
  snapshot: Snapshot;
  onChat: (id: string, sourceId?: string) => void;
}) {
  const [filter, setFilter] = useState("all");
  const tools = snapshot.tools.filter(
    (tool) =>
      filter === "all" ||
      (filter === "error"
        ? tool.isError === true ||
          (tool.exitCode != null && tool.exitCode !== 0) ||
          ["failed", "error"].includes(tool.status)
        : tool.kind === filter),
  );
  return (
    <>
      <div className="panel-toolbar">
        <div className="tabs">
          {[
            ["all", "全部记录"],
            ["error", "失败与异常"],
            ["mcp", "MCP"],
          ].map(([key, text]) => (
            <button
              key={key}
              className={filter === key ? "active" : ""}
              onClick={() => setFilter(key)}
            >
              {text}
            </button>
          ))}
        </div>
        <span className="subtle-note">
          {formatNumber(tools.length)} 条工具事实
        </span>
      </div>
      <section className="card table-card">
        {tools.length ? (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>工具名称</th>
                  <th>类型</th>
                  <th>记录状态</th>
                  <th>退出码 / 错误</th>
                  <th>时长</th>
                  <th>会话</th>
                </tr>
              </thead>
              <tbody>
                {tools.map((tool, index) => (
                  <tr key={`${tool.id}-${index}`}>
                    <td>
                      <span className="tool-name">
                        <Icon name="terminal" size={17} />
                        <code>{tool.name}</code>
                      </span>
                    </td>
                    <td>
                      <Badge>{tool.kind}</Badge>
                    </td>
                    <td>
                      <Badge
                        tone={
                          isSuccess(tool.status)
                            ? "mint"
                            : ["failed", "error"].includes(tool.status)
                              ? "rose"
                              : "amber"
                        }
                      >
                        {statusLabel(tool.status)}
                      </Badge>
                    </td>
                    <td>
                      {tool.exitCode != null
                        ? `exit ${tool.exitCode}`
                        : tool.isError != null
                          ? tool.isError
                            ? "来源标记错误"
                            : "来源未标记错误"
                          : "未提供"}
                    </td>
                    <td>{duration(tool.durationMs)}</td>
                    <td>
                      <button
                        className="icon-button"
                        aria-label="打开工具所属聊天"
                        onClick={() => onChat(tool.threadId, tool.sourceId)}
                      >
                        <Icon name="external" size={16} />
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <Empty
            icon="terminal"
            title="暂无匹配的工具记录"
            detail="仅保留名称、类型、结果标记和明确时长，不保存命令、参数或输出正文。"
          />
        )}
      </section>
      <div className="notice">
        <Icon name="info" size={18} />
        <span>
          工具正常退出仅表示该工具的结果；不等同于测试通过、代码正确或业务目标达成。并行工具时长不会相加为回合等待时间。
        </span>
      </div>
    </>
  );
}

export function InboxPanel({
  snapshot,
  onChat,
  action,
  initialFilter = "all",
  navigationRevision,
  onNavigationMounted,
  onFilterChange,
}: {
  snapshot: Snapshot;
  onChat: (id: string, sourceId?: string) => void;
  action: Action;
  initialFilter?: "all" | "unread" | "other";
  navigationRevision?: string;
  onNavigationMounted?: (revision: string) => void;
  onFilterChange?: () => void;
}) {
  const [filter, setFilter] = useState(initialFilter);
  useEffect(() => {
    if (navigationRevision && filter === "unread")
      onNavigationMounted?.(navigationRevision);
  }, [navigationRevision, filter, onNavigationMounted]);
  const unread = snapshot.inbox.filter((item) => !item.read).length;
  const entries = snapshot.inbox.filter(
    (item) =>
      filter === "all" ||
      (filter === "unread" ? !item.read : !isSuccess(item.status)),
  );
  const [confirmClear, setConfirmClear] = useState(false);
  return (
    <>
      <div className="panel-toolbar">
        <div className="tabs">
          {[
            ["all", "全部回音"],
            ["unread", `未读 ${unread}`],
            ["other", "失败 / 中断 / 未知"],
          ].map(([key, text]) => (
            <button
              key={key}
              className={filter === key ? "active" : ""}
              onClick={() => {
                onFilterChange?.();
                setFilter(key as "all" | "unread" | "other");
              }}
            >
              {text}
            </button>
          ))}
        </div>
        <div className="button-group">
          <button
            className="button secondary small"
            disabled={!unread}
            onClick={() => void action("mark_read")}
          >
            <Icon name="check" size={16} />
            全部已读
          </button>
          <button
            className="icon-button"
            title="清理本地收件箱"
            aria-label="清理本地收件箱"
            onClick={() => setConfirmClear(true)}
          >
            <Icon name="trash" size={17} />
          </button>
        </div>
      </div>
      <section className="card inbox-card">
        {entries.length ? (
          entries.map((entry, index) => (
            <article
              className={`inbox-item ${entry.read ? "" : "unread"}`}
              key={entry.id}
            >
              <div
                className={`event-icon ${isSuccess(entry.status) ? "success" : "warning"}`}
              >
                <Icon
                  name={isSuccess(entry.status) ? "check" : "info"}
                  size={21}
                />
              </div>
              <div className="inbox-body">
                <div className="inbox-title">
                  <h3>
                    {snapshot.settings.privacyMode
                      ? `私密会话 ${index + 1}`
                      : entry.title || "回合结束记录"}
                  </h3>
                  {!entry.read && <span className="unread-dot" />}
                  <Badge tone={isSuccess(entry.status) ? "mint" : "amber"}>
                    {entry.kind === "budget"
                      ? "预算阈值提醒"
                      : statusLabel(entry.status)}
                  </Badge>
                </div>
                <p>
                  {entry.kind === "budget"
                    ? "本地软预算提醒，不改变 AI 工具执行。"
                    : "来源记录的根回合结果，可返回原会话查看。"}
                </p>
                <time>
                  {dateTime(entry.createdAt)} · {entry.read ? "已读" : "待查看"}
                </time>
              </div>
              <div className="inbox-actions">
                {!entry.read && (
                  <button
                    className="text-button"
                    onClick={() => void action("mark_read", { id: entry.id })}
                  >
                    标为已读
                  </button>
                )}
                {entry.threadId && (
                  <button
                    className="button secondary small"
                    onClick={() => onChat(entry.threadId, entry.sourceId)}
                  >
                    {entry.sourceId ? "返回聊天" : "复制会话标识"}
                    <Icon name="external" size={14} />
                  </button>
                )}
              </div>
            </article>
          ))
        ) : (
          <Empty
            icon="inbox"
            title={filter === "unread" ? "所有回音都看过啦" : "还没有新的回音"}
            detail="根回合结束后会记录在这里。历史补导只进入历史视图，不补弹旧通知。"
          />
        )}
      </section>
      <p className="subtle-note page-note">
        “正常结束”不是业务验收；没有明确结果的结束记录显示“结果未知”。
      </p>
      {confirmClear && (
        <Modal title="清理本地收件箱" onClose={() => setConfirmClear(false)}>
          <p>
            这会清理宠物保存的提醒记录，并保留清理边界。Codex
            的原始会话和日志不会被修改。
          </p>
          <div className="modal-actions">
            <button
              className="button secondary"
              onClick={() => setConfirmClear(false)}
            >
              取消
            </button>
            <button
              className="button danger"
              onClick={async () => {
                await action("clear_inbox");
                setConfirmClear(false);
              }}
            >
              清理提醒
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}

export function HealthPanel({
  snapshot,
  action,
  onRefresh,
}: {
  snapshot: Snapshot;
  action: Action;
  onRefresh: () => void;
}) {
  const health = snapshot.health;
  const count = issueCount(health.issues);
  const [from, setFrom] = useState("2026-09-15");
  const [sourceError, setSourceError] = useState<string | null>(null);
  async function chooseSource(id?: string) {
    if (!isDesktop) {
      setSourceError(
        "浏览器预览不能选择本机目录；请在 Windows 桌面应用登记来源。",
      );
      return;
    }
    try {
      const path = await invoke<string | null>("choose_source_directory");
      if (path)
        await action(
          id ? "update_source" : "add_source",
          id ? { id, path } : { path },
        );
    } catch (error) {
      setSourceError(String(error));
    }
  }
  return (
    <>
      <section className="card health-summary">
        <span
          className={`health-emblem ${count || issueCount(health.conflicts) ? "has-issues" : ""}`}
        >
          <Icon name="heart" size={30} />
        </span>
        <div>
          <h2>
            {count || issueCount(health.conflicts)
              ? "有一些数据需要检查"
              : health.lastSuccessfulScan
                ? "本机采集正在工作"
                : "等待首次成功采集"}
          </h2>
          <p>来源的健康与任务状态独立记录；没有新事件不会被判定为任务失败。</p>
        </div>
        <button className="button secondary" onClick={onRefresh}>
          <Icon name="refresh" size={16} />
          刷新视图
        </button>
      </section>
      <div className="health-grid">
        {[
          ["最后成功检查", dateTime(health.lastSuccessfulScan)],
          ["最后来源事件", dateTime(health.lastSourceEvent)],
          ["已发现文件", formatNumber(health.files)],
          ["未知模型记录", formatNumber(health.unknownModels)],
        ].map(([label, value]) => (
          <section className="card small-metric" key={label}>
            <span>{label}</span>
            <strong>{value}</strong>
          </section>
        ))}
      </div>
      {snapshot.desktop && (
        <section className="card desktop-health">
          <div className="section-heading">
            <h2>桌面与读取通道</h2>
            <Badge>各通道独立降级</Badge>
          </div>
          <dl className="metric-list">
            <div>
              <dt>交互恢复快捷键</dt>
              <dd>
                {snapshot.desktop.recoveryShortcut || "Ctrl + Alt + M"} ·{" "}
                {snapshot.desktop.shortcutAvailable === false
                  ? "注册失败，请使用托盘找回"
                  : "已注册"}
              </dd>
            </div>
            <div>
              <dt>会话目录只读通道</dt>
              <dd>
                {snapshot.desktop.directory?.status ?? "尚未读取"} ·{" "}
                {snapshot.desktop.directory?.count ?? "未提供"} 个目录条目
              </dd>
            </div>
            <div>
              <dt>目录最后成功</dt>
              <dd>{dateTime(snapshot.desktop.directory?.lastSuccessAt)}</dd>
            </div>
            <div>
              <dt>系统提醒请求</dt>
              <dd>
                {(
                  {
                    submitted: "已提交给系统",
                    failed: "提交失败",
                    suppressed: "已抑制",
                  } as Record<string, string>
                )[snapshot.desktop.notifications?.status ?? ""] ?? "尚未请求"}
              </dd>
            </div>
            <div>
              <dt>通知最后尝试</dt>
              <dd>{dateTime(snapshot.desktop.notifications?.lastAttemptAt)}</dd>
            </div>
          </dl>
          {[
            snapshot.desktop.collectorError,
            snapshot.desktop.shellError,
            snapshot.desktop.watcherError,
            snapshot.desktop.directory?.error,
            snapshot.desktop.notifications?.error,
          ]
            .filter(Boolean)
            .map((error, index) => (
              <p className="subtle-note" key={index}>
                {error}
              </p>
            ))}
          <p className="subtle-note">
            “已提交给系统”仅表示请求已提交，不证明操作系统已送达；来源账本与收件箱继续保留事实。
          </p>
        </section>
      )}
      <section className="card">
        <div className="section-heading">
          <h2>来源与能力</h2>
          <button
            className="button secondary small"
            onClick={() => void chooseSource()}
          >
            <Icon name="plus" size={15} />
            登记 Codex 来源
          </button>
        </div>
        {sourceError && <div className="notice inset">{sourceError}</div>}
        {snapshot.sources?.length ? (
          snapshot.sources.map((source, index) => (
            <SourceDetails
              key={source.id}
              source={source}
              index={index}
              snapshot={snapshot}
              chooseSource={chooseSource}
              action={action}
            />
          ))
        ) : (
          <div className="source-card">
            <span className="source-logo">C</span>
            <div>
              <h3>
                Codex <Badge>本机 JSONL</Badge>
              </h3>
              <p className="wrap-path">
                {snapshot.settings.privacyMode
                  ? "来源路径已隐藏"
                  : health.sourceRoot || "尚未发现来源目录"}
              </p>
            </div>
            <Badge tone={health.lastSuccessfulScan ? "mint" : "amber"}>
              {health.lastSuccessfulScan ? "已接入" : "等待采集"}
            </Badge>
          </div>
        )}
        <p className="subtle-note">
          迁移目录保留同一来源身份；登记新来源会创建新身份。跨来源疑点会从可信总计排除，不按目录名自动合并。
        </p>
        <div className="future-sources">
          <span>Claude Code</span>
          <Badge>后续扩展 · 未接入</Badge>
          <span>Gemini CLI</span>
          <Badge>后续扩展 · 未接入</Badge>
        </div>
      </section>
      <div className="bottom-grid">
        <section className="card">
          <div className="section-heading">
            <h2>覆盖范围与历史</h2>
            <Icon name="clock" size={18} />
          </div>
          <dl className="metric-list">
            <div>
              <dt>历史补导状态</dt>
              <dd>
                {health.history?.status
                  ? ({
                      running: "补导中",
                      paused: "已暂停",
                      cancelled: "已取消",
                      idle: "空闲",
                    }[health.history.status] ?? "状态未提供")
                  : "状态未提供"}
              </dd>
            </div>
            <div>
              <dt>待补导文件</dt>
              <dd>
                {health.history?.pendingFiles == null
                  ? "未提供"
                  : formatNumber(health.history.pendingFiles)}
              </dd>
            </div>
            <div>
              <dt>已覆盖开始</dt>
              <dd>
                {health.coverageFrom
                  ? String(health.coverageFrom).slice(0, 10)
                  : "尚未记录"}
              </dd>
            </div>
            <div>
              <dt>已覆盖结束</dt>
              <dd>
                {health.coverageTo
                  ? String(health.coverageTo).slice(0, 10)
                  : "尚未记录"}
              </dd>
            </div>
            <div>
              <dt>不支持的压缩 / 来源文件</dt>
              <dd>{health.unsupportedFiles ?? "未提供"}</dd>
            </div>
            <div>
              <dt>身份冲突</dt>
              <dd>{issueCount(health.conflicts)}</dd>
            </div>
          </dl>
          <p className="subtle-note">
            历史只代表已采集来源。身份未确认和冲突事实不会混入可信总计。
            暂停或取消仅作用于历史补导，实时新增事件继续采集。
            {health.history?.status === "cancelled" &&
              " 本次补导已取消；需要重新选择日期并点击“补导历史”，不会通过恢复按钮重新启动。"}
          </p>
          <form
            className="history-form"
            onSubmit={(event) => {
              event.preventDefault();
              void action("import_history", { from });
            }}
          >
            <label>
              补导起始日期
              <input
                type="date"
                required
                value={from}
                onChange={(event) => setFrom(event.target.value)}
              />
            </label>
            <button className="button secondary" type="submit">
              补导历史
            </button>
          </form>
          <div className="button-group">
            <button
              className="text-button"
              disabled={health.history?.status !== "running"}
              onClick={() => void action("pause_history")}
            >
              <Icon name="pause" size={14} />
              暂停补导
            </button>
            <button
              className="text-button"
              disabled={health.history?.status !== "paused"}
              onClick={() => void action("resume_history")}
            >
              <Icon name="play" size={14} />
              恢复补导
            </button>
            <button
              className="text-button"
              disabled={
                !["running", "paused"].includes(health.history?.status ?? "")
              }
              onClick={() => void action("cancel_history")}
            >
              <Icon name="close" size={14} />
              取消补导
            </button>
          </div>
        </section>
        <section className="card">
          <div className="section-heading">
            <h2>解析与契约问题</h2>
            <Badge tone={count ? "amber" : "mint"}>{count} 项</Badge>
          </div>
          {Array.isArray(health.issues) && health.issues.length ? (
            <div className="issue-list">
              {health.issues.slice(0, 12).map((issue, index) => {
                const data =
                  typeof issue === "object" && issue
                    ? (issue as Record<string, unknown>)
                    : { message: issue };
                return (
                  <div key={index}>
                    <Icon name="info" size={17} />
                    <span>
                      <strong>{String(data.kind ?? "来源问题")}</strong>
                      <p>{String(data.message ?? "解析异常")}</p>
                      {!snapshot.settings.privacyMode && data.file != null && (
                        <small className="wrap-path">{String(data.file)}</small>
                      )}
                    </span>
                    <Badge>{String(data.count ?? 1)}</Badge>
                  </div>
                );
              })}
            </div>
          ) : (
            <Empty
              icon="check"
              title="当前没有已记录的解析问题"
              detail="这表示问题索引为空，不代表覆盖所有来源或历史。"
            />
          )}
        </section>
      </div>
    </>
  );
}

export function SettingsPanel({
  snapshot,
  save,
  action,
  onAccountRefresh,
  pending,
  onResetPet,
}: {
  snapshot: Snapshot;
  save: (settings: Settings) => Promise<void>;
  action: Action;
  onAccountRefresh: () => void;
  pending: boolean;
  onResetPet: () => void;
}) {
  const extensions = usePetExtensionCapabilities();
  const skins = useLocalPetSkins(localPetSkinClient, extensions.enabled);
  const [tab, setTab] = useState("pet"),
    [budgetOpen, setBudgetOpen] = useState(false),
    [scope, setScope] = useState("daily"),
    [target, setTarget] = useState(""),
    [threshold, setThreshold] = useState("2000000"),
    [confirmClear, setConfirmClear] = useState(false);
  const settings = snapshot.settings;
  function row(key: keyof Settings, title: string, detail: string, unavailable = false) {
    return (
      <div className="setting-row" key={key}>
        <div>
          <strong>{title}</strong>
          <p>{detail}</p>
        </div>
        <Toggle
          label={title}
          checked={!unavailable && !!settings[key]}
          disabled={pending || unavailable}
          onChange={(value) => void save({ [key]: value })}
        />
      </div>
    );
  }
  async function addBudget(event: React.FormEvent) {
    event.preventDefault();
    const value = Number(threshold);
    if (!Number.isSafeInteger(value) || value <= 0) return;
    const thread = ["thread", "rootTurn"].includes(scope)
      ? parseIdentity(target)
      : null;
    if (["thread", "rootTurn"].includes(scope) && !thread) return;
    await action("save_budget", {
      scope,
      target: scope === "daily" ? null : (thread?.id ?? target),
      sourceId: thread?.sourceId ?? null,
      threshold: value,
      enabled: true,
    });
    setBudgetOpen(false);
  }
  const budgetTitle = (budget: Budget) =>
    budget.scope === "daily"
      ? "本地日预算"
      : `${budget.scope === "rootTurn" ? "根任务累计" : budget.scope === "project" ? "项目" : "会话"} · ${settings.privacyMode ? "已隐藏" : budget.scope === "project" ? (snapshot.projects.find((project) => project.id === budget.target)?.name ?? "未显示的项目") : budget.scope === "rootTurn" ? `${budget.target?.slice(0, 12) ?? "未指定"} · ${sourceLabel(budget.sourceId ?? undefined, snapshot.sources)}` : (snapshot.sessions.find((session) => session.id === budget.target && session.sourceId === budget.sourceId)?.title ?? "未显示的会话")}`;
  return (
    <>
      <div className="tabs settings-tabs">
        {[
          ["pet", "桌面伙伴"],
          ["notifications", "提醒与隐私"],
          ["budgets", "软预算"],
          ["data", "数据与账户"],
        ].map(([key, text]) => (
          <button
            key={key}
            className={tab === key ? "active" : ""}
            onClick={() => setTab(key)}
          >
            {text}
          </button>
        ))}
      </div>
      {tab === "pet" && (
        <>
          <section className="card">
            <div className="section-heading">
              <h2>小柚的桌面</h2>
              <Badge tone="mint">原创内置角色</Badge>
            </div>
            {row(
              "petVisible",
              "显示桌面伙伴",
              "隐藏角色后仍继续记账；也可以从托盘找回。",
            )}
            {row(
              "alwaysOnTop",
              "窗口置顶",
              "让悬浮宠物保持在其他普通窗口上方。",
            )}
            {row(
              "clickThrough",
              "整体鼠标穿透",
              "开启后从托盘或 Ctrl + Alt + M 恢复交互，也可临时交互 15 秒。",
            )}
            {row("alphaHitTest", "透明区域命中", "保守保留角色轮廓与文字按钮；待新版 Windows 原生验收，当前仅开发入口可用。", !extensions.enabled)}
            <TemporaryInteractionSettings enabled={extensions.enabled} />
            {row(
              "reducedMotion",
              "减少动画",
              "使用静态姿态，文字与角标继续显示当前事实。",
            )}
            {row(
              "startAtLogin",
              "登录 Windows 时启动",
              "默认关闭；使用本应用自己的启动入口。",
            )}
            <div className="setting-row">
              <div>
                <strong>角色大小</strong>
                <p>缩放角色与交互区域，范围 50%–200%。</p>
              </div>
              <label className="inline-select">
                <select
                  aria-label="角色大小"
                  value={Number(settings.petScale ?? 1)}
                  onChange={(event) =>
                    void save({ petScale: Number(event.target.value) })
                  }
                >
                  {[0.5, 0.75, 1, 1.25, 1.5, 2].map((scale) => (
                    <option value={scale} key={scale}>
                      {scale * 100}%
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <div className="setting-row">
              <div>
                <strong>一键找回宠物</strong>
                <p>恢复交互，并把宠物放回当前可见工作区。</p>
              </div>
              <button className="button secondary" onClick={onResetPet}>
                找回小柚
                <Icon name="cat" size={17} />
              </button>
            </div>
          </section>
          <CompanionSettings
            settings={settings}
            save={save}
            pending={pending}
          />
          {!extensions.enabled && <p className="muted">本地皮肤、透明命中与临时交互待新版 Windows 原生验收，当前尚未开放。</p>}
          <LocalPetSkinSettings model={skins} client={localPetSkinClient} disabled={pending || !extensions.enabled} />
          <section className="card character-info">
            <Icon name="cat" size={26} />
            <div>
              <h3>{builtinManifest.attribution}</h3>
              <p>
                本地透明 PNG 序列帧 · AI 辅助生成资源，随本应用分发 ·
                {builtinManifest.variants?.tap.length
                  ? "五种语义动作、待机姿态、轻点回应与独立静态兜底"
                  : "五种语义动作与独立静态兜底"}
              </p>
              <small>不读取屏幕、不收集键盘内容、不额外调用模型。</small>
            </div>
          </section>
          <section className="card">
            <div className="section-heading">
              <h2>工作联动与关心语</h2>
              <Icon name="heart" size={20} />
            </div>
            <div className="setting-row">
              <div>
                <strong>工作联动关心语</strong>
                <p>依据本地新事件轻声回应；关闭后仍保留摸摸姿态与事实角标。</p>
              </div>
              <Toggle
                label="工作联动关心语"
                checked={settings.workCareEnabled !== false}
                disabled={pending}
                onChange={(value) => void save({ workCareEnabled: value })}
              />
            </div>
            <div className="setting-row">
              <div>
                <strong>关心语气</strong>
                <p>两种风格交替时，按实际显示的每句轮换。</p>
              </div>
              <label className="inline-select">
                <select
                  aria-label="关心语气"
                  value={settings.careTone ?? "mixed"}
                  disabled={pending}
                  onChange={(event) =>
                    void save({
                      careTone: event.target.value as Settings["careTone"],
                    })
                  }
                >
                  <option value="mixed">两种风格交替</option>
                  <option value="gentle">温柔克制</option>
                  <option value="playful">轻轻卖萌</option>
                </select>
              </label>
            </div>
            <div className="setting-row">
              <div>
                <strong>持续活动关心间隔</strong>
                <p>
                  只计算可持续观察到的进行中回合；隐藏、锁屏或免打扰后重新观察。
                </p>
              </div>
              <label className="inline-select">
                <select
                  aria-label="持续活动关心间隔"
                  value={settings.careIntervalMinutes ?? 25}
                  disabled={pending}
                  onChange={(event) =>
                    void save({
                      careIntervalMinutes: Number(event.target.value) as
                        | 25
                        | 45
                        | 60,
                    })
                  }
                >
                  {[25, 45, 60].map((minutes) => (
                    <option value={minutes} key={minutes}>
                      {minutes} 分钟
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <p className="subtle-note">
              短句全部内置，不读取屏幕或输入内容，不调用模型。只保留一个短气泡，历史、重启与免打扰期间的关心语不补播。
            </p>
          </section>
        </>
      )}
      {tab === "notifications" && (
        <section className="card">
          <div className="section-heading">
            <h2>安静，也能知道进展</h2>
            <Icon name="moon" size={20} />
          </div>
          {row("muted", "静音", "默认开启。关闭后，仅已支持的提示可使用声音。")}
          {row(
            "doNotDisturb",
            "免打扰",
            "不主动弹窗或发声；收件箱继续记录，短提示不补播。",
          )}
          {row(
            "privacyMode",
            "隐私模式",
            "隐藏项目名称、会话标题和目录；必要统计保留。",
          )}
          <div className="setting-row">
            <div>
              <strong>定时免打扰</strong>
              <p>按 Asia/Shanghai 计算；开始与结束相同表示不设定时段。</p>
            </div>
            <div className="time-range">
              <input
                aria-label="免打扰开始时间"
                type="time"
                value={String(settings.dndStart ?? "22:00")}
                disabled={pending}
                onChange={(event) =>
                  void save({
                    dndStart: event.target.value,
                    dndEnd: String(settings.dndEnd ?? "22:00"),
                  })
                }
              />
              <span>—</span>
              <input
                aria-label="免打扰结束时间"
                type="time"
                value={String(settings.dndEnd ?? "22:00")}
                disabled={pending}
                onChange={(event) =>
                  void save({
                    dndStart: String(settings.dndStart ?? "22:00"),
                    dndEnd: event.target.value,
                  })
                }
              />
            </div>
          </div>
          <div className="notice inset">
            <Icon name="shield" size={17} />
            <span>
              提醒只读事实，不自动审批、停止任务、切换模型或修改 AI 工具配置。
            </span>
          </div>
        </section>
      )}
      {tab === "budgets" && (
        <section className="card">
          <div className="section-heading">
            <div>
              <h2>给用量一个温柔的边界</h2>
              <p>按本机总 token 计量，只提醒，不干预任务。</p>
            </div>
            <button
              className="button primary small"
              onClick={() => setBudgetOpen(true)}
            >
              <Icon name="plus" size={16} />
              添加预算
            </button>
          </div>
          {snapshot.budgets.length ? (
            snapshot.budgets.map((budget) => (
              <div className="budget-row" key={budget.id}>
                <span className="circle-icon">
                  <Icon name="sparkles" size={20} />
                </span>
                <div className="budget-info">
                  <strong>{budgetTitle(budget)}</strong>
                  <span>
                    {formatNumber(budget.usedTokens)} /{" "}
                    {formatNumber(budget.threshold)} token
                  </span>
                  {budget.usedTokens != null ? (
                    <div className="progress-track">
                      <span
                        style={{
                          width: `${Math.min(100, ((budget.usedTokens ?? 0) / budget.threshold) * 100)}%`,
                        }}
                      />
                    </div>
                  ) : (
                    <small className="subtle-note">
                      归属尚不可确定，未按 0 计量
                    </small>
                  )}
                </div>
                <Toggle
                  label="启用此预算"
                  checked={budget.enabled}
                  onChange={(enabled) =>
                    void action("save_budget", { ...budget, enabled })
                  }
                />
                <button
                  className="icon-button"
                  aria-label="删除预算"
                  onClick={() =>
                    void action("delete_budget", { id: budget.id })
                  }
                >
                  <Icon name="trash" size={17} />
                </button>
              </div>
            ))
          ) : (
            <Empty
              icon="sparkles"
              title="还没有设置预算"
              detail="可以按本地日、项目或会话添加 token 阈值。"
            />
          )}
          <p className="subtle-note">
            同一周期每档位提醒一次。根任务累计自身及明确关联的子代理，不在午夜重置；其余预算按上海当地日计量。账户官方额度单独计算。
          </p>
        </section>
      )}
      {tab === "data" && (
        <>
          <AccountCard
            snapshot={snapshot}
            onRefresh={onAccountRefresh}
            pending={pending}
          />
          <section className="card">
            <div className="section-heading">
              <h2>本地数据管理</h2>
              <Icon name="shield" size={18} />
            </div>
            <div className="setting-row">
              <div>
                <strong>账本保留天数</strong>
                <p>仅清理宠物自己保存的数据，保留时间边界以避免立即回灌。</p>
              </div>
              <select
                aria-label="数据保留天数"
                value={Number(settings.retentionDays ?? 90)}
                onChange={(event) =>
                  void save({ retentionDays: Number(event.target.value) })
                }
              >
                {[7, 30, 90, 180, 365, 3650].map((days) => (
                  <option value={days} key={days}>
                    {days === 3650 ? "10 年" : `${days} 天`}
                  </option>
                ))}
              </select>
            </div>
            <div className="setting-row">
              <div>
                <strong>应用保留策略</strong>
                <p>按上面的设置清理超期账本。</p>
              </div>
              <button
                className="button secondary"
                onClick={() => void action("apply_retention")}
              >
                清理超期数据
              </button>
            </div>
            <div className="setting-row">
              <div>
                <strong>清理宠物自身数据</strong>
                <p>清理用量、提醒与账户快照；不会删除 Codex 源文件。</p>
              </div>
              <button
                className="button danger-outline"
                onClick={() => setConfirmClear(true)}
              >
                清理本地账本
              </button>
            </div>
          </section>
          <div className="notice">
            <Icon name="shield" size={18} />
            <span>
              对话正文、命令参数、工具输出、凭据与完整账户响应不进入宠物数据库。CSV
              对公式起始字符进行转义。
            </span>
          </div>
        </>
      )}
      {budgetOpen && (
        <Modal title="添加软预算" onClose={() => setBudgetOpen(false)}>
          <form
            onSubmit={(event) => void addBudget(event)}
            className="modal-form"
          >
            <label>
              预算范围
              <select
                value={scope}
                onChange={(event) => {
                  setScope(event.target.value);
                  setTarget("");
                }}
              >
                <option value="daily">本地日 · 每日重新计量</option>
                <option value="project">项目</option>
                <option value="thread">会话 · 按当地日</option>
                <option value="rootTurn">根任务累计 · 含明确关联子代理</option>
              </select>
            </label>
            {scope !== "daily" && (
              <label>
                {scope === "project"
                  ? "选择项目"
                  : scope === "rootTurn"
                    ? "选择根任务"
                    : "选择会话"}
                <select
                  required
                  value={target}
                  onChange={(event) => setTarget(event.target.value)}
                >
                  <option value="">请选择…</option>
                  {scope === "project"
                    ? snapshot.projects.map((project, index) => (
                        <option key={project.id} value={project.id}>
                          {settings.privacyMode
                            ? `项目 ${index + 1}`
                            : project.name}
                        </option>
                      ))
                    : scope === "rootTurn"
                      ? rootTurnOptions(snapshot.turns, snapshot.sessions).map(
                          (root, index) => (
                            <option
                              key={identityValue(root)}
                              value={identityValue(root)}
                            >
                              {settings.privacyMode
                                ? `根任务 ${index + 1}`
                                : `${root.id.slice(0, 12)} · ${sourceLabel(root.sourceId, snapshot.sources)}`}
                            </option>
                          ),
                        )
                      : snapshot.sessions.map((session, index) => (
                          <option
                            key={`${session.sourceId}:${session.id}`}
                            value={
                              session.sourceId
                                ? identityValue({
                                    sourceId: session.sourceId,
                                    id: session.id,
                                  })
                                : ""
                            }
                            disabled={!session.sourceId}
                          >
                            {settings.privacyMode
                              ? `会话 ${index + 1}`
                              : session.title}
                          </option>
                        ))}
                </select>
              </label>
            )}
            <label>
              提醒阈值（总 token）
              <input
                type="number"
                step="1"
                min="1"
                max="9007199254740991"
                required
                value={threshold}
                onChange={(event) => setThreshold(event.target.value)}
              />
            </label>
            <p className="subtle-note">
              本机可信总量达到阈值后提醒，不会改变工具执行。
            </p>
            <div className="modal-actions">
              <button
                className="button secondary"
                type="button"
                onClick={() => setBudgetOpen(false)}
              >
                取消
              </button>
              <button className="button primary" type="submit">
                创建预算
              </button>
            </div>
          </form>
        </Modal>
      )}
      {confirmClear && (
        <Modal title="清理宠物本地账本" onClose={() => setConfirmClear(false)}>
          <p>
            用量、回合、工具、提醒与账户快照会被清理。应用会保存清理水位，避免下次扫描立即导入刚清理的历史。需要恢复历史时，可在数据健康中明确补导。
          </p>
          <p>AI 工具的源文件不会被修改。</p>
          <div className="modal-actions">
            <button
              className="button secondary"
              onClick={() => setConfirmClear(false)}
            >
              取消
            </button>
            <button
              className="button danger"
              onClick={async () => {
                await action("clear_data");
                setConfirmClear(false);
              }}
            >
              确认清理自身数据
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}
