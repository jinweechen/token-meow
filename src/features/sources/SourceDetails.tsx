import type { Snapshot, SourceInstance } from "../../shared/contracts";
import { dateTime, formatNumber } from "../../shared/format";
import { Badge, Toggle } from "../../shared/UI";
import {
  capabilityFields,
  capabilityGranularity,
  capabilityRuntime,
} from "../../shared/sourceCapabilities";

export function SourceDetails({
  source,
  index,
  snapshot,
  chooseSource,
  action,
}: {
  source: SourceInstance;
  index: number;
  snapshot: Snapshot;
  chooseSource: (id: string) => Promise<void>;
  action: (name: string, payload?: Record<string, unknown>) => Promise<void>;
}) {
  const primary = source.id === snapshot.primarySourceId;
  const channels =
    snapshot.channels?.filter((channel) => channel.sourceId === source.id) ??
    [];
  const capabilities =
    snapshot.sourceCapabilities?.filter(
      (report) => report.sourceId === source.id,
    ) ?? [];
  const client = source.clientKind === "codex" ? "Codex" : "未知客户端";
  return (
    <article
      className="source-instance"
      aria-label={`${client} 来源 ${index + 1}`}
    >
      <div className="source-card">
        <span className="source-logo">
          {source.clientKind === "codex" ? "C" : "?"}
        </span>
        <div>
          <h3>
            {client} · 来源 {index + 1}{" "}
            <Badge>{source.environment || "环境未提供"}</Badge>{" "}
            <Badge tone={source.runtime === "可用" ? "mint" : "amber"}>
              {source.runtime ?? "运行态未提供"}
            </Badge>
          </h3>
          <p className="wrap-path">
            {snapshot.settings.privacyMode ? "来源路径已隐藏" : source.root}
          </p>
          <small className="subtle-note">
            稳定身份 {source.id.slice(0, 12)} ·{" "}
            {primary ? "主账户来源；快照单独读取" : "账户能力：未启用独立采集"}
          </small>
        </div>
        <div className="source-actions">
          <button
            className="text-button"
            onClick={() => void chooseSource(source.id)}
          >
            更改目录
          </button>
          {primary ? (
            <Badge>当前主账户来源</Badge>
          ) : (
            <button
              className="text-button"
              disabled={!source.enabled || source.clientKind !== "codex"}
              onClick={() =>
                void action("set_primary_source", { id: source.id })
              }
            >
              设为主账户来源
            </button>
          )}
          <Toggle
            label={`启用 ${client} 来源 ${index + 1}`}
            checked={source.enabled}
            onChange={(enabled) =>
              void action("update_source", { id: source.id, enabled })
            }
          />
        </div>
      </div>
      <dl className="source-health">
        <div>
          <dt>最近检查</dt>
          <dd>{dateTime(source.health?.checkedAt)}</dd>
        </div>
        <div>
          <dt>本轮检查文件</dt>
          <dd>
            {source.health?.files == null
              ? "未提供"
              : formatNumber(source.health.files)}
          </dd>
        </div>
        <div>
          <dt>待检查文件</dt>
          <dd>
            {source.health?.pendingFiles == null
              ? "未提供"
              : formatNumber(source.health.pendingFiles)}
          </dd>
        </div>
        <div>
          <dt>本轮问题</dt>
          <dd>
            {source.health?.issues == null
              ? "未提供"
              : formatNumber(source.health.issues)}
          </dd>
        </div>
      </dl>
      <details className="source-channel-details">
        <summary>查看此来源的采集通道与能力</summary>
        <dl className="source-health">
          <div>
            <dt>最近无错检查</dt>
            <dd>{dateTime(source.health?.lastSuccessfulCheck)}</dd>
          </div>
          <div>
            <dt>最后来源事件</dt>
            <dd>{dateTime(source.health?.lastEvent)}</dd>
          </div>
          <div>
            <dt>已观测事件范围</dt>
            <dd>
              {dateTime(source.health?.observedFrom)} 至{" "}
              {dateTime(source.health?.observedTo)}
            </dd>
          </div>
          <div>
            <dt>未知模型记录</dt>
            <dd>
              {source.health?.unknownModels == null
                ? "未提供"
                : formatNumber(source.health.unknownModels)}
            </dd>
          </div>
          <div>
            <dt>不支持的来源文件</dt>
            <dd>
              {source.health?.unsupportedFiles == null
                ? "未提供"
                : formatNumber(source.health.unsupportedFiles)}
            </dd>
          </div>
        </dl>
        {channels.length ? (
          <div className="source-channels">
            {channels.map((channel) => (
              <p key={channel.id}>
                <strong>
                  {channel.kind === "jsonl" ? "JSONL 本地文件" : "未知通道"}
                </strong>
                <Badge
                  tone={source.enabled && channel.enabled ? "mint" : "amber"}
                >
                  {!source.enabled
                    ? "已随来源停用"
                    : channel.enabled === true
                      ? "配置已启用"
                      : channel.enabled === false
                        ? "通道未启用"
                        : "状态未提供"}
                </Badge>
              </p>
            ))}
          </div>
        ) : (
          <p className="subtle-note">此来源尚未提供通道登记信息。</p>
        )}
        <p className="subtle-note">
          通道登记与来源启用共同决定是否启用；来源运行态只表示目录与采集状况，未自动探测当前客户端版本。能力声明仅覆盖所列版本及证据，缺项为未验证。账户能力通过独立
          app-server 通道，当前状态只取本来源绑定的账户快照。
        </p>
        {capabilities.length ? (
          capabilities.map((report, reportIndex) => (
            <div
              className="capability-grid"
              key={`${report.channelId ?? "unknown"}:${reportIndex}`}
            >
              {capabilityFields.map(([key, label]) => {
                const field = report[key];
                return (
                  <div className="capability-item" key={key}>
                    <strong>{label}</strong>
                    <span>{field?.support ?? "支持情况未提供"}</span>
                    <small>
                      能力运行态：
                      {capabilityRuntime(key, field, source, snapshot)}
                      {(key === "quota" || key === "accountUsage") &&
                        "（独立账户快照）"}
                    </small>
                    <small>
                      来源运行态：
                      {field?.sourceRuntime ?? source.runtime ?? "未提供"}
                    </small>
                    <small>
                      支持版本：{field?.supportedVersion ?? "未验证"}
                    </small>
                    <small>实测版本：{field?.testedVersion ?? "未验证"}</small>
                    <small>采集通道：{field?.channel ?? "未验证"}</small>
                    <small>
                      粒度：{capabilityGranularity(field?.granularity)}
                    </small>
                    <small>最近验证：{field?.verifiedAt ?? "未验证"}</small>
                    <small>{field?.versionPolicy ?? "其他版本未验证"}</small>
                    <small>
                      前置条件：
                      {field?.prerequisites?.length
                        ? field.prerequisites.join("；")
                        : "未验证"}
                    </small>
                    <small>
                      验证范围：{field?.verificationScope ?? "未验证"}
                    </small>
                    {field?.note && <small>{field.note}</small>}
                    <small>
                      证据：
                      {snapshot.settings.privacyMode
                        ? "隐私模式已隐藏"
                        : field?.evidenceRefs?.length
                          ? field.evidenceRefs.join("；")
                          : (field?.evidence ?? "未验证")}
                    </small>
                  </div>
                );
              })}
            </div>
          ))
        ) : (
          <p className="subtle-note">
            此来源尚未提供能力声明；不会借用其它来源的能力。
          </p>
        )}
      </details>
      {!source.enabled && (
        <p className="subtle-note">此来源已停用；先启用后可设为主账户来源。</p>
      )}
    </article>
  );
}
