import type { PetPresentation, Settings } from "../../shared/contracts";
import { isSuccess } from "../../shared/format";
import { PromptWatermark } from "./promptWatermark";
import type { Prompt } from "./presenter";

export type CareTone = "gentle" | "playful" | "mixed";
export type CareKind =
  | "start"
  | "complete"
  | "failed"
  | "interrupted"
  | "unknown"
  | "budget"
  | "ongoing"
  | "tap"
  | "tap-busy"
  | "tap-uncertain";
export interface CareBubble {
  id: number;
  kind: CareKind;
  tone: Exclude<CareTone, "mixed">;
  text: string;
  expiresAt: number;
}
export const CARE_LIMITS = {
  lifetime: 8_000,
  automaticCooldown: 60_000,
  manualCooldown: 3_000,
  observationGap: 15_000,
} as const;
const phrases: Record<CareKind, [string, string]> = {
  start: ["回合开始了，我在这里陪着。", "开工啦～小柚在旁边陪你。"],
  complete: [
    "这个回合已正常结束，辛苦了。",
    "这个回合正常完成啦～给你一朵小花。",
  ],
  failed: [
    "回合失败了，先看看记录，慢慢处理。",
    "这次没跑通。别急，小柚陪你看看记录。",
  ],
  interrupted: [
    "回合已中断，先确认一下当前进度。",
    "回合停下来了～先看看记录再继续吧。",
  ],
  unknown: [
    "回合已结束，结果还不确定，请查看记录。",
    "回合结束啦，结果还没确认，先看看记录吧。",
  ],
  budget: [
    "已达到本地软预算阈值，可以查看用量。",
    "本地软预算到提醒线啦～看一眼用量吧。",
  ],
  ongoing: [
    "回合仍在进行，喝口水，也让目光歇一会儿。",
    "回合还在跑～喝口水、伸个懒腰，小柚陪着。",
  ],
  tap: ["收到你的摸摸了，我在这里。", "摸摸收到～今天也一起慢慢来。"],
  "tap-busy": [
    "收到摸摸。回合仍在进行，我安静陪着。",
    "摸摸收到～任务还在跑，小柚安静守着。",
  ],
  "tap-uncertain": [
    "收到摸摸。回合状态还待确认，可以看看记录。",
    "摸摸收到～状态还没确认，先看看记录吧。",
  ],
};
function priority(kind: CareKind) {
  return ["failed", "interrupted", "unknown", "budget"].includes(kind)
    ? 4
    : kind === "complete"
      ? 3
      : kind.startsWith("tap")
        ? 2
        : 1;
}
function promptKind(prompt: Prompt): CareKind | null {
  if (prompt.kind === "budget") return "budget";
  if (!["turn_completed", "completion"].includes(prompt.kind ?? ""))
    return null;
  if (isSuccess(prompt.status)) return "complete";
  if (["failed", "error"].includes(prompt.status)) return "failed";
  if (["interrupted", "cancelled", "canceled"].includes(prompt.status))
    return "interrupted";
  return "unknown";
}
/** Global source identity, never a page filter or text included in a phrase. */
export function careSourceScope(presentation: PetPresentation): string {
  return presentation.sourceScopeRevision;
}

/** Local ephemeral text only. No scheduler, network, title/path interpolation or backlog. */
export class WorkCareController {
  private watermark = new PromptWatermark();
  private initialized = false;
  private scope = "";
  private active = 0;
  private uncertain = 0;
  private observedAt: number | null = null;
  private activeSince: number | null = null;
  private lastOngoing: number | null = null;
  private automaticAt = -Infinity;
  private automaticPriority = 0;
  private manualAt = -Infinity;
  private serial = 0;
  private mixedIndex = 0;
  private bubble: CareBubble | null = null;
  private destroyed = false;
  observe(
    snapshot: PetPresentation,
    ready: boolean,
    blocked: boolean,
    now: number,
  ): CareBubble | null {
    if (this.destroyed) return null;
    this.expire(now);
    const scope = careSourceScope(snapshot);
    const enabled = snapshot.settings.workCareEnabled !== false;
    if (!ready || blocked || !enabled) {
      this.watermark.consume(snapshot.prompts, now, true);
      this.initialized = false;
      this.bubble = null;
      this.activeSince = this.observedAt = this.lastOngoing = null;
      this.scope = scope;
      return null;
    }
    const baseline =
      !this.initialized ||
      scope !== this.scope ||
      this.observedAt === null ||
      now < this.observedAt ||
      now - this.observedAt > CARE_LIMITS.observationGap;
    const previousActive = this.active;
    this.active = snapshot.activity.activeTurns;
    this.uncertain = snapshot.activity.uncertainTurns;
    if (
      this.bubble &&
      ((this.active === 0 &&
        ["start", "ongoing", "tap-busy"].includes(this.bubble.kind)) ||
        (this.uncertain === 0 && this.bubble.kind === "tap-uncertain"))
    )
      this.bubble = null;
    this.observedAt = now;
    if (baseline) {
      this.scope = scope;
      this.initialized = true;
      this.activeSince = this.active > 0 ? now : null;
      this.lastOngoing = null;
      this.bubble = null;
      this.watermark.reset();
      this.watermark.consume(snapshot.prompts, now, true);
      return null;
    }
    const kinds = this.watermark
      .consume(snapshot.prompts, now, false)
      .map(promptKind)
      .filter((kind): kind is CareKind => kind !== null);
    if (this.active <= 0) this.activeSince = this.lastOngoing = null;
    else if (previousActive <= 0) this.activeSince = now;
    const interval = [25, 45, 60].includes(
      Number(snapshot.settings.careIntervalMinutes),
    )
      ? Number(snapshot.settings.careIntervalMinutes) * 60_000
      : 25 * 60_000;
    if (kinds.length)
      this.show(
        kinds.sort((a, b) => priority(b) - priority(a))[0],
        snapshot.settings,
        now,
        false,
      );
    else if (previousActive === 0 && this.active > 0)
      this.show("start", snapshot.settings, now, false);
    else if (
      this.activeSince !== null &&
      now - (this.lastOngoing ?? this.activeSince) >= interval
    ) {
      this.lastOngoing = now;
      this.show("ongoing", snapshot.settings, now, false);
    }
    return this.bubble;
  }
  tap(settings: Settings, allowed: boolean, now: number): CareBubble | null {
    if (
      this.observedAt === null ||
      now < this.observedAt ||
      now - this.observedAt > CARE_LIMITS.observationGap
    ) {
      // Retained UI snapshots can outlive a failed collector request. A manual
      // gesture must not turn that old activity into a current factual claim.
      this.initialized = false;
      this.activeSince = this.lastOngoing = this.observedAt = null;
      this.bubble = null;
      return null;
    }
    if (
      this.destroyed ||
      !allowed ||
      !this.initialized ||
      settings.workCareEnabled === false
    )
      return this.bubble;
    this.expire(now);
    if (
      now - this.manualAt < CARE_LIMITS.manualCooldown ||
      (this.bubble && priority(this.bubble.kind) >= 3)
    )
      return this.bubble;
    this.manualAt = now;
    this.show(
      this.active > 0
        ? "tap-busy"
        : this.uncertain > 0
          ? "tap-uncertain"
          : "tap",
      settings,
      now,
      true,
    );
    return this.bubble;
  }
  private show(
    kind: CareKind,
    settings: Settings,
    now: number,
    manual: boolean,
  ) {
    const level = priority(kind);
    if (this.bubble && level <= priority(this.bubble.kind)) return;
    if (
      !manual &&
      now - this.automaticAt < CARE_LIMITS.automaticCooldown &&
      level <= this.automaticPriority
    )
      return;
    if (!manual) {
      this.automaticAt = now;
      this.automaticPriority = level;
    }
    const configured = settings.careTone ?? "mixed";
    const tone =
      configured === "gentle" || configured === "playful"
        ? configured
        : this.mixedIndex++ % 2 === 0
          ? "gentle"
          : "playful";
    this.bubble = {
      id: ++this.serial,
      kind,
      tone,
      text: phrases[kind][tone === "gentle" ? 0 : 1],
      expiresAt: now + CARE_LIMITS.lifetime,
    };
  }
  current(now: number) {
    this.expire(now);
    return this.bubble;
  }
  private expire(now: number) {
    if (this.bubble && now >= this.bubble.expiresAt) this.bubble = null;
  }
  dismiss() {
    this.bubble = null;
  }
  suspend() {
    this.initialized = false;
    this.activeSince = this.lastOngoing = this.observedAt = null;
    this.bubble = null;
    this.watermark.reset();
  }
  destroy() {
    this.destroyed = true;
    this.bubble = null;
    this.watermark.reset();
  }
}
