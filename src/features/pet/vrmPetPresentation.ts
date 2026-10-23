import type { PetPresentation, PetState } from "../../shared/contracts";
import { ANIMATION_LIMITS, AnimationController } from "./animationController";
import { PET_REACTIONS, PoseController, type PetReaction } from "./poseController";
import { presentPet, promptText } from "./presenter";
import { PromptWatermark } from "./promptWatermark";

export interface VrmPetInput {
  modelId: string;
  presentation: PetPresentation | null;
  ready: boolean;
  blocked: boolean;
  reducedMotion: boolean;
}

export interface VrmPetFrame {
  domain: string | null;
  action: PetState;
  reaction: PetReaction | null;
  promptId: string | null;
  text: string;
  paused: boolean;
}

const createAnimation = () => new AnimationController(() => 1_500, true);
const createPoses = () => new PoseController({ idle: [], tap: PET_REACTIONS.map(value => value.frame) }, () => 1_500);

export function vrmPetDomain(input: Pick<VrmPetInput, "modelId" | "presentation" | "ready">): string | null {
  const epoch = (input.presentation as (PetPresentation & { runtimeEpoch?: string }) | null)?.runtimeEpoch ?? null;
  return input.presentation && input.ready && input.presentation.sourceScopeRevision
    ? JSON.stringify([input.modelId, epoch, input.presentation.sourceScopeRevision]) : null;
}

/** 只管理有限的表现和提示水位；不构造活动事实，也不拥有窗口移动许可。 */
export class VrmPetPresentation {
  private animation = createAnimation();
  private poses = createPoses();
  private watermark = new PromptWatermark();
  private domain: string | null = null;
  private blocked = true;
  private delivered = new Set<string>();
  private lastNow: number | null = null;
  private promptAfter = -Infinity;
  private disposed = false;
  private frame: VrmPetFrame = { domain: null, action: "idle", reaction: null, promptId: null, text: "正在读取本地活动…", paused: true };

  observe(input: VrmPetInput, now: number): VrmPetFrame {
    const intent = presentPet(input.presentation, input.ready);
    const domain = vrmPetDomain(input);
    const changed = domain !== this.domain;
    if (changed) {
      this.animation.destroy(); this.poses.destroy();
      this.animation = createAnimation(); this.poses = createPoses();
      this.watermark.reset(); this.delivered.clear();
      this.domain = domain;
      this.blocked = true;
    }
    const rollback = this.lastNow !== null && now < this.lastNow;
    this.lastNow = now;
    const paused = this.disposed || !input.presentation || !input.ready || !domain || input.blocked || input.reducedMotion;
    this.animation.suppress(paused);
    if (rollback) { this.interrupt(); this.blocked = true; }
    // 原生恢复可能先于 React 提交暂停期间的批次；按恢复时刻阻止迟到的旧提示。
    if (this.blocked && !paused) this.promptAfter = now;
    if (input.presentation && input.ready) {
      for (const prompt of this.watermark.consume(input.presentation.prompts, now, paused || changed || this.blocked || rollback))
        if (prompt.createdAt > this.promptAfter) this.animation.enqueue(prompt, now);
    }
    this.blocked = paused;
    let next = this.animation.tick(intent.action, now);
    // 模型加载没有时间上限；不能把一直没有画出的旧提示留到加载完成后再确认。
    if (next.prompt && !this.delivered.has(next.prompt.id) && now - next.prompt.createdAt > ANIMATION_LIMITS.ttl) {
      this.animation.interrupt();
      next = this.animation.tick(intent.action, now);
    }
    const eligible = !paused && next.action === "idle" && !next.prompt && intent.active === 0 && intent.uncertain === 0;
    const pose = this.poses.tick(eligible, now, false);
    const reaction = pose ? PET_REACTIONS.find(value => value.frame === pose.frame)?.id ?? null : null;
    this.frame = {
      domain,
      action: paused ? intent.action : next.action,
      reaction: paused ? null : reaction,
      promptId: paused ? null : next.prompt?.id ?? null,
      text: next.prompt && !paused
        ? `${promptText(next.prompt)}${intent.active > 0 ? ` · ${intent.active} 个回合仍活动中` : ""}` : intent.text,
      paused,
    };
    return this.frame;
  }

  tap(input: VrmPetInput, now: number, reaction?: unknown): VrmPetFrame {
    const frame = this.observe(input, now);
    const intent = presentPet(input.presentation, input.ready);
    this.poses.tap(!frame.paused && frame.action === "idle" && !frame.promptId && intent.active === 0 && intent.uncertain === 0, now, reaction);
    return this.observe(input, now);
  }

  /** 调用方必须在当前模型实际绘制、提交对应状态后再次校验可见性与域。 */
  acknowledge(domain: string | null, promptId: string, now: number): boolean {
    if (this.disposed || this.frame.paused || !domain || domain !== this.domain || this.frame.promptId !== promptId || this.delivered.has(promptId)) return false;
    this.delivered.add(promptId);
    if (this.delivered.size > 128) this.delivered.delete(this.delivered.values().next().value!);
    this.animation.startPlayback(promptId, now);
    return true;
  }

  interrupt(): void { this.animation.interrupt(); this.poses.interrupt(); }
  dispose(): void { this.disposed = true; this.animation.destroy(); this.poses.destroy(); }
}
