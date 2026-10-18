export const MOTION_PRESENTATION_LIMITS = { playingMs: 2400, movingMs: 8000 } as const;
export const MAX_RETIRED_MOTION_EPOCHS = 32;
type Phase = "paused" | "idle" | "playing" | "moving";
type ActivePhase = "playing" | "moving";
export interface CompanionMotionState {
  runtimeEpoch: string;
  revision: string;
  motionGeneration: string;
  phase: Phase;
  capabilities: { roaming: boolean };
  motion: { direction: "left" | "right"; remainingMs: number } | null;
}
export interface MotionPresentation {
  runtimeEpoch: string;
  motionGeneration: string;
  phase: ActivePhase;
  direction: "left" | "right" | null;
  /** A deadline in the caller's local clock, never an account/native timestamp. */
  expiresAt: number;
  remainingMs: number;
}
const decimal = (value: unknown): value is string =>
  typeof value === "string" && /^(0|[1-9]\d{0,19})$/.test(value);
function envelope(value: unknown): value is Pick<CompanionMotionState,
  "runtimeEpoch" | "revision" | "motionGeneration" | "phase"> & Partial<CompanionMotionState> {
  if (!value || typeof value !== "object") return false;
  const state = value as Partial<CompanionMotionState>;
  return typeof state.runtimeEpoch === "string" && state.runtimeEpoch.length > 0 &&
    decimal(state.revision) && decimal(state.motionGeneration) &&
    ["paused", "idle", "playing", "moving"].includes(state.phase ?? "");
}
/** Motion extension only: combine this with the complete companion validator. */
export function validCompanionMotion(value: unknown): value is CompanionMotionState {
  if (!envelope(value) || typeof value.capabilities?.roaming !== "boolean") return false;
  if (value.phase !== "moving") return value.motion === null;
  const motion = value.motion;
  return value.capabilities.roaming && !!motion &&
    (motion.direction === "left" || motion.direction === "right") &&
    Number.isSafeInteger(motion.remainingMs) && motion.remainingMs > 0 &&
    motion.remainingMs <= MOTION_PRESENTATION_LIMITS.movingMs;
}

/** Pure visual lease owner. It never moves a window, emits facts or owns a timer.
 * Bind an epoch only after CompanionStateReader's authoritative snapshot accepts
 * it. Old async observations cannot rebind that epoch by themselves.
 */
export class CompanionMotionPresentation {
  private epoch: string | null = null;
  private retiredEpochs = new Set<string>();
  private epochCapacityExhausted = false;
  private revision: bigint | null = null;
  private generation: bigint | null = null;
  private retired = new Set<ActivePhase>();
  private lease: Omit<MotionPresentation, "remainingMs"> | null = null;
  private lastNow: number | null = null;

  bindEpoch(epoch: string, now: number): boolean {
    if (this.epochCapacityExhausted) return false;
    if (!epoch || !this.clock(now)) return false;
    if (epoch === this.epoch) return true; // Same-epoch refresh cannot reopen a lease.
    if (this.retiredEpochs.has(epoch)) return false;
    if (this.epoch && this.retiredEpochs.size >= MAX_RETIRED_MOTION_EPOCHS) {
      // Never forget a retired domain to admit another one. This owner remains
      // closed until its view lifetime ends and a fresh owner is constructed.
      this.epochCapacityExhausted = true;
      this.clear();
      return false;
    }
    if (this.epoch) this.retiredEpochs.add(this.epoch);
    this.epoch = epoch;
    this.revision = this.generation = null;
    this.retired.clear();
    this.lease = null;
    return true;
  }

  observe(value: unknown, now: number): boolean {
    if (this.epochCapacityExhausted) return false;
    if (!this.clock(now)) return false;
    this.expire(now);
    if (!envelope(value)) { this.clear(); return false; }
    if (value.runtimeEpoch !== this.epoch) return false;
    const revision = BigInt(value.revision), generation = BigInt(value.motionGeneration);
    if ((this.revision !== null && revision <= this.revision) ||
        (this.generation !== null && generation < this.generation)) return false;
    this.revision = revision;
    if (!validCompanionMotion(value)) { this.clear(); return false; }
    if (generation !== this.generation) {
      this.generation = generation;
      this.retired.clear();
      this.lease = null;
    } else if (this.lease && this.lease.phase !== value.phase) {
      this.clear();
    }
    if (value.phase === "idle" || value.phase === "paused") { this.clear(); return true; }
    if (this.retired.has(value.phase)) return false;
    const duration = value.phase === "playing" ? MOTION_PRESENTATION_LIMITS.playingMs : value.motion!.remainingMs;
    const expiresAt = Math.min(this.lease?.expiresAt ?? Infinity, now + duration);
    this.lease = {
      runtimeEpoch: value.runtimeEpoch, motionGeneration: value.motionGeneration,
      phase: value.phase, direction: value.phase === "moving" ? value.motion!.direction : null, expiresAt,
    };
    return true;
  }

  read(now: number): MotionPresentation | null {
    if (this.epochCapacityExhausted) return null;
    if (!this.clock(now)) return null;
    this.expire(now);
    return this.lease ? Object.freeze({ ...this.lease, remainingMs: this.lease.expiresAt - now }) : null;
  }

  /** Invalidation closes this generation/phase; later revisions cannot revive it. */
  clear() {
    if (this.lease) this.retired.add(this.lease.phase);
    this.lease = null;
  }

  private expire(now: number) {
    if (this.lease && now >= this.lease.expiresAt) this.clear();
  }
  private clock(now: number) {
    if (!Number.isFinite(now) || now < 0 || now > Number.MAX_SAFE_INTEGER - MOTION_PRESENTATION_LIMITS.movingMs) {
      this.clear();
      return false;
    }
    if (this.lastNow !== null && now < this.lastNow) this.clear();
    this.lastNow = now;
    return true;
  }
}
