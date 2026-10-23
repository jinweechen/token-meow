import type { InboxEvent } from "../../shared/contracts";
import { dateTime, isSuccess, statusLabel } from "../../shared/format";
import { Badge, Icon } from "../../shared/UI";

export function inboxEventSuccess(event: InboxEvent) {
  return !["subscription_plan", "quota_eta", "budget"].includes(event.kind) && isSuccess(event.status ?? "unknown");
}
export function inboxEventLabel(event: InboxEvent) {
  return event.kind === "subscription_plan" ? "使用计划提醒" : event.kind === "quota_eta" ? "官方额度风险" : event.kind === "budget" ? "预算阈值提醒" : statusLabel(event.status ?? "unknown");
}
function title(event: InboxEvent, privacy: boolean, index = 0) {
  return privacy ? `${event.kind === "subscription_plan" ? "私密计划" : event.kind === "quota_eta" ? "私密额度" : event.kind === "budget" ? "私密预算" : "私密会话"} ${index + 1}`
    : event.title || (event.kind === "subscription_plan" ? "使用计划提醒" : event.kind === "quota_eta" ? "官方额度风险" : event.kind === "budget" ? "本地软预算提醒" : "回合结束记录");
}
export function InboxEventRow({ event, index, privacy, canViewPlan, onRead, onChat, onPlan }: {
  event: InboxEvent; index: number; privacy: boolean; canViewPlan: boolean;
  onRead: (id: string) => void; onChat: (id: string, sourceId?: string) => void; onPlan?: () => void;
}) {
  const success = inboxEventSuccess(event);
  const plan = event.kind === "subscription_plan";
  const quota = event.kind === "quota_eta";
  const detail = plan || quota ? (!privacy && event.message ? event.message : quota ? "近期用量可能在重置前触限，可在订阅与计划中查看。" : "用户设定的使用计划达到提醒目标，可在订阅与计划中查看。")
    : event.kind === "budget" ? "本地软预算提醒，不改变 AI 工具执行。" : "来源记录的根回合结果，可返回原会话查看。";
  return <article className={`inbox-item ${event.read ? "" : "unread"}`}>
    <div className={`event-icon ${success ? "success" : "warning"}`}><Icon name={success ? "check" : "info"} size={21} /></div>
    <div className="inbox-body"><div className="inbox-title"><h3>{title(event, privacy, index)}</h3>{!event.read && <span className="unread-dot" />}<Badge tone={success ? "mint" : "amber"}>{inboxEventLabel(event)}</Badge></div><p>{detail}</p><time>{dateTime(event.createdAt)} · {event.read ? "已读" : "待查看"}</time></div>
    <div className="inbox-actions">
      {!event.read && <button className="text-button" onClick={() => onRead(event.id)}>标为已读</button>}
      {(plan || quota) && canViewPlan && onPlan && <button className="button secondary small" onClick={onPlan}>{quota ? "查看额度" : "查看计划"}<Icon name="arrow" size={14} /></button>}
      {!plan && !quota && event.threadId && <button className="button secondary small" onClick={() => onChat(event.threadId, event.sourceId)}>{event.sourceId ? "返回聊天" : "复制会话标识"}<Icon name="external" size={14} /></button>}
    </div>
  </article>;
}

export function RecentInboxEvent({ event, privacy, onChat, onInbox }: {
  event: InboxEvent; privacy: boolean; onChat: (id: string, sourceId?: string) => void; onInbox: () => void;
}) {
  const success = inboxEventSuccess(event);
  return <button className="recent-event" onClick={() => { if (["subscription_plan", "quota_eta"].includes(event.kind) || !event.threadId) onInbox(); else onChat(event.threadId, event.sourceId); }}>
    <span className={`event-icon ${success ? "success" : "warning"}`}><Icon name={success ? "check" : "info"} size={16} /></span>
    <span><strong>{title(event, privacy)}</strong><small>{inboxEventLabel(event)} · {dateTime(event.createdAt, true)}</small></span><Icon name="chevron" size={15} />
  </button>;
}
