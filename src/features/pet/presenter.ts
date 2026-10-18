import type { PetPresentation, PetState } from "../../shared/contracts";
import { issueCount, isSuccess, statusLabel } from "../../shared/format";
export interface PetIntent {
  action: PetState;
  text: string;
  badge: string | null;
  health: string | null;
  active: number;
  uncertain: number;
}
export interface Prompt {
  id: string;
  status: string;
  createdAt: number;
  kind?: string;
}
export function presentPet(presentation: PetPresentation | null, ready = true): PetIntent {
  if (!presentation || !ready)
    return {
      action: "idle",
      text: presentation ? "活动数据待更新" : "正在读取本地活动…",
      badge: null,
      health: null,
      active: 0,
      uncertain: 0,
    };
  const active = presentation.activity.activeTurns,
    uncertain = presentation.activity.uncertainTurns;
  const health =
    issueCount(presentation.health.issues) > 0 ||
    issueCount(presentation.health.conflicts) > 0
      ? "采集有待检查项"
      : presentation.health.lastSuccessfulScan
        ? null
        : "尚未连接来源";
  const unread = presentation.unreadCount;
  const dnd =
    presentation.settings.effectiveDoNotDisturb ?? presentation.settings.doNotDisturb;
  return {
    action:
      active > 0 ? "active" : uncertain > 0 ? "alert" : dnd ? "sleep" : "idle",
    text:
      active > 0
        ? `${active} 个回合活动中`
        : uncertain > 0
          ? `${uncertain} 个回合状态待确认`
          : dnd
            ? "免打扰中，安静陪伴"
            : "在这里，陪你慢慢创造",
    badge: unread ? `${unread} 条待查看` : null,
    health,
    active,
    uncertain,
  };
}
export function promptAction(prompt: Prompt): PetState {
  return isSuccess(prompt.status) ? "complete" : "alert";
}
export function promptText(prompt: Prompt): string {
  return prompt.kind === "budget"
    ? "已达到本地软预算提醒阈值"
    : statusLabel(prompt.status);
}
