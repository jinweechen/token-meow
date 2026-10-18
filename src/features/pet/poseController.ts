import type { DesktopCompanionState } from "../../shared/desktopCompanion";

export const POSE_LIMITS = {
  idleInterval: 18_000,
  idleDuration: 2_400,
  tapDuration: 1_800,
  tapCooldown: 3_000,
} as const;
export interface CutePose {
  frame: number;
  kind: "idle" | "tap";
}

/** Natural native play expiry may finish visually; fresh input or lost authority may not. */
export function allowsCompanionPose(state: DesktopCompanionState | null | undefined, enabled = true) {
  return !!state && enabled && state.capabilities.systemIdle &&
    (state.phase === "playing" || state.phase === "idle") && state.pauseReason === null &&
    state.inputIdleMs !== null && state.inputIdleMs >= 120_000;
}

/** A single visual pose, driven only by the existing visible animation clock. */
export class PoseController {
  private current: CutePose | null = null;
  private origin: "automatic" | "companion" | "tap" | null = null;
  private instance = 0;
  private playbackStarted = false;
  private companionGeneration: string | null = null;
  private until = 0;
  private nextIdle = 0;
  private lastTap = -Infinity;
  private idleIndex = 0;
  private tapIndex = 0;
  private destroyed = false;
  constructor(
    private variants: { idle: number[]; tap: number[] } = { idle: [], tap: [] },
    private duration: (frame: number, kind: CutePose["kind"]) => number = (_frame, kind) =>
      kind === "idle" ? POSE_LIMITS.idleDuration : POSE_LIMITS.tapDuration,
    private awaitPlayback = false,
  ) {}
  private start(frame: number, kind: CutePose["kind"], now: number, origin: NonNullable<PoseController["origin"]>) {
    this.current = { frame, kind };
    this.origin = origin;
    this.instance++;
    this.playbackStarted = !this.awaitPlayback;
    this.until = this.awaitPlayback ? Infinity : now + this.duration(frame, kind);
  }
  playbackKey() { return this.current ? this.instance : null; }
  /** Asset loading does not consume a finite pose's reviewed playback duration. */
  startPlayback(instance: number, now: number) {
    if (!this.current || this.instance !== instance || this.playbackStarted) return;
    this.playbackStarted = true;
    this.until = now + this.duration(this.current.frame, this.current.kind);
    this.nextIdle = Math.max(this.nextIdle, now + POSE_LIMITS.idleInterval);
  }
  finishPlayback(instance: number) {
    if (this.current && this.instance === instance) this.interrupt();
  }
  /** A native generation grants one visual action, never a renewable motion lease. */
  play(eligible: boolean, generation: string, frame: number, now: number): boolean {
    if (generation === this.companionGeneration) return false;
    this.companionGeneration = generation;
    if (this.destroyed || !eligible || this.current?.kind === "tap") return false;
    this.start(frame, "idle", now, "companion");
    return true;
  }
  stopCompanion() {
    if (this.origin === "companion") this.interrupt();
  }
  tick(eligible: boolean, now: number, allowAutomaticIdle = true): CutePose | null {
    if (this.destroyed || !eligible) {
      this.interrupt();
      return null;
    }
    if (this.current && now >= this.until) {
      this.current = null;
      this.origin = null;
    }
    if (!allowAutomaticIdle) {
      if (this.origin === "automatic") {
        this.current = null;
        this.origin = null;
      }
      this.nextIdle = 0;
      return this.current;
    }
    if (!this.nextIdle) this.nextIdle = now + POSE_LIMITS.idleInterval;
    if (!this.current && now >= this.nextIdle && this.variants.idle.length) {
      this.start(this.variants.idle[this.idleIndex++ % this.variants.idle.length], "idle", now, "automatic");
      this.nextIdle = now + POSE_LIMITS.idleInterval;
    }
    return this.current;
  }
  tap(eligible: boolean, now: number): boolean {
    if (this.destroyed || !eligible) {
      this.interrupt();
      return false;
    }
    if (
      !this.variants.tap.length ||
      now - this.lastTap < POSE_LIMITS.tapCooldown
    )
      return false;
    this.start(this.variants.tap[this.tapIndex++ % this.variants.tap.length], "tap", now, "tap");
    this.lastTap = now;
    this.nextIdle = now + POSE_LIMITS.idleInterval;
    return true;
  }
  interrupt() {
    this.current = null;
    this.origin = null;
    this.until = 0;
    this.nextIdle = 0;
  }
  destroy() {
    this.destroyed = true;
    this.interrupt();
  }
}

