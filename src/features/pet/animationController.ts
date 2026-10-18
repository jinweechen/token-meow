import type { PetState } from "../../shared/contracts";
import { promptAction, type Prompt } from "./presenter";
export const ANIMATION_LIMITS = {
  queue: 8,
  ttl: 10_000,
  completion: 2_000,
  cooldown: 3_000,
} as const;
export class AnimationController {
  private queue: Prompt[] = [];
  private seen = new Map<string, number>();
  private current: Prompt | null = null;
  private started = 0;
  private playbackStarted = false;
  private until = 0;
  private cooldownUntil = 0;
  private suppressed = false;
  private destroyed = false;
  constructor(
    private duration: (action: PetState) => number = () => ANIMATION_LIMITS.completion,
    private awaitPlayback = false,
  ) {}
  private start(prompt: Prompt, now: number) {
    this.current = prompt;
    this.started = now;
    this.playbackStarted = !this.awaitPlayback;
    this.until = this.awaitPlayback ? Infinity : now + this.duration(promptAction(prompt));
  }
  startPlayback(promptId: string, now: number) {
    if (!this.current || this.current.id !== promptId || this.playbackStarted) return;
    this.playbackStarted = true;
    this.started = now;
    this.until = now + this.duration(promptAction(this.current));
  }
  finishPlayback(promptId: string) {
    if (this.current?.id === promptId) this.until = 0;
  }
  enqueue(prompt: Prompt, now: number): boolean {
    if (
      this.destroyed ||
      this.suppressed ||
      this.seen.has(prompt.id) ||
      now - prompt.createdAt > ANIMATION_LIMITS.ttl ||
      prompt.createdAt > now + 1000
    )
      return false;
    this.seen.set(prompt.id, now);
    this.prune(now);
    if (
      this.current &&
      promptAction(prompt) === "alert" &&
      promptAction(this.current) === "complete"
    ) {
      this.start(prompt, now);
      return true;
    }
    if (this.queue.length >= ANIMATION_LIMITS.queue) return false;
    this.queue.push(prompt);
    return true;
  }
  private prune(now: number): void {
    this.queue = this.queue.filter(
      (p) => now - p.createdAt <= ANIMATION_LIMITS.ttl,
    );
    for (const [id, time] of this.seen)
      if (now - time > 86_400_000 || this.seen.size > 512) this.seen.delete(id);
  }
  suppress(value: boolean): void {
    this.suppressed = value;
    if (value) {
      this.queue = [];
      this.current = null;
      this.until = 0;
    }
  }
  interrupt(): void {
    this.queue = [];
    this.current = null;
    this.until = 0;
  }
  tick(
    base: PetState,
    now: number,
  ): {
    action: PetState;
    prompt: Prompt | null;
    phase: "start" | "loop" | "end" | "static";
  } {
    if (this.destroyed || this.suppressed)
      return { action: base, prompt: null, phase: "static" };
    this.prune(now);
    if (this.current && now >= this.until) {
      this.current = null;
      this.cooldownUntil = now + ANIMATION_LIMITS.cooldown;
    }
    if (!this.current && now >= this.cooldownUntil && this.queue.length) {
      this.queue.sort(
        (a, b) =>
          Number(promptAction(b) === "alert") -
          Number(promptAction(a) === "alert"),
      );
      this.start(this.queue.shift()!, now);
    }
    return {
      action: this.current ? promptAction(this.current) : base,
      prompt: this.current,
      phase: this.current
        ? now - this.started < 250
          ? "start"
          : this.until - now < 250
            ? "end"
            : "loop"
        : "loop",
    };
  }
  diagnostics() {
    return {
      queued: this.queue.length,
      seen: this.seen.size,
      active: !!this.current,
      destroyed: this.destroyed,
    };
  }
  destroy(): void {
    this.interrupt();
    this.seen.clear();
    this.destroyed = true;
  }
}
