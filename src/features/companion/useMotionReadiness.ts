import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "../../shared/bridge";
import type { PetPresentation, PetPulse, PetState } from "../../shared/contracts";

export const MOTION_READINESS_HEARTBEAT_MS = 500;
export const MOTION_READINESS_NATIVE_LEASE_MS = 1500;
const PRESENTATION_FRESH_MS = 15000;

export function isPetMotionRenderer(desktop: boolean, decorative: boolean, search: string) {
  return desktop && !decorative && new URLSearchParams(search).get("window") === "pet";
}

/** A presentation veto only. Native independently verifies work and input. */
export function allowPetMotionHeartbeat(presentation: (PetPresentation & Partial<Pick<PetPulse, "asOf" | "runtimeEpoch">>) | null, local: {
  ready: boolean; paused: boolean; sourceScopeRevision: string | null;
  hasPrompt: boolean; tap: boolean; action: PetState; interactionBlocked: boolean;
}, now: number) {
  if (!presentation || !local.ready || local.paused || local.hasPrompt || local.tap ||
      local.action !== "idle" || local.interactionBlocked ||
      !local.sourceScopeRevision || local.sourceScopeRevision !== presentation.sourceScopeRevision ||
      presentation.activity.activeTurns !== 0 || presentation.activity.uncertainTurns !== 0 ||
      presentation.settings.petVisible === false || presentation.settings.reducedMotion === true ||
      (presentation.settings.effectiveDoNotDisturb ?? presentation.settings.doNotDisturb)) return false;
  const header = presentation as Partial<PetPulse>;
  const age = now - Date.parse(header.asOf ?? "");
  return typeof header.runtimeEpoch === "string" && header.runtimeEpoch.length > 0 &&
    Number.isFinite(now) && Number.isFinite(age) && age >= -1000 && age <= PRESENTATION_FRESH_MS;
}

export interface MotionReadinessInput {
  /** A local restriction only; never a replacement for native work facts. */
  ready: boolean;
  gaitAvailable: boolean;
  gaitReady: boolean;
  motionGeneration?: string | null;
  motionDirection?: "left" | "right" | null;
  /** 原生已自然结束移动段时，只释放步态，不重置系统空闲计时。 */
  nativeMotionPhase?: "idle" | "playing" | "moving" | "paused" | "expired";
  visible?: boolean;
  reducedMotion?: boolean;
  paused?: boolean;
}
export interface MotionReadinessPayload {
  rendererEpoch: string;
  token: string;
  sequence: string;
  ready: boolean;
  gaitAvailable: boolean;
  gaitReady: boolean;
  motionGeneration: string | null;
  motionDirection: "left" | "right" | null;
}
type Flags = Pick<MotionReadinessPayload, "ready" | "gaitAvailable" | "gaitReady" | "motionGeneration" | "motionDirection">;
type Candidate = { flags: Flags; at: number; revocation?: boolean };
const closed = (gaitAvailable = false): Flags => ({ ready: false, gaitAvailable, gaitReady: false,
  motionGeneration: null, motionDirection: null });
const same = (left: Flags, right: Flags) => left.ready === right.ready &&
  left.gaitAvailable === right.gaitAvailable && left.gaitReady === right.gaitReady &&
  left.motionGeneration === right.motionGeneration && left.motionDirection === right.motionDirection;
const generation = (value: unknown): value is string => typeof value === "string" &&
  /^(0|[1-9]\d{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;

/** One IPC request plus one latest candidate. Caller supplies its existing tick.
 * Tauri promises cannot cancel a delivered native command: a revocation replaces
 * queued permission and drains immediately after the current request settles.
 * A stalled request cannot renew the independent native 1500 ms lease.
 */
export class PetMotionReadinessOwner {
  readonly rendererEpoch: string;
  private sequence = 0n;
  private inFlight = false;
  private pending: Candidate | null = null;
  private desired: Flags | null = null;
  private lastSentAt: number | null = null;
  private lastNow: number | null = null;
  private disposed = false;
  private revoking = false;
  private token: string | null = null;
  private registration = 0;

  constructor(private readonly options: {
    register: (rendererEpoch: string) => Promise<string>;
    send: (readiness: MotionReadinessPayload) => Promise<boolean>;
    now?: () => number;
    rendererEpoch?: string;
  }) {
    this.rendererEpoch = options.rendererEpoch ?? crypto.randomUUID();
  }

  update(input: MotionReadinessInput, at = this.now()) {
    if (this.disposed) return;
    const clockValid = this.acceptClock(at);
    const identityValid = generation(input.motionGeneration) &&
      (input.motionDirection === "left" || input.motionDirection === "right");
    const ready = clockValid && input.ready === true && input.visible !== false &&
      input.reducedMotion !== true && input.paused !== true && !(input.gaitReady === true && !identityValid);
    const gaitReady = input.gaitAvailable === true && input.gaitReady === true && identityValid;
    const flags: Flags = {
      ready,
      gaitAvailable: input.gaitAvailable === true,
      gaitReady,
      motionGeneration: gaitReady ? input.motionGeneration! : null,
      motionDirection: gaitReady ? input.motionDirection! : null,
    };
    if (!ready) { this.revoke(at, input.gaitAvailable === true); return; }
    // Do not erase a queued revocation with a newly eligible frame. The next
    // existing animation tick can reacquire after the false command settles.
    if (this.revoking) return;
    const nativeEnded = input.nativeMotionPhase === "idle" || input.nativeMotionPhase === "playing" || input.nativeMotionPhase === "expired";
    if (this.desired && ((this.desired.gaitAvailable && !flags.gaitAvailable) ||
        (this.desired.gaitReady && !flags.gaitReady && !nativeEnded))) {
      const restriction = closed(flags.gaitAvailable);
      this.desired = restriction;
      this.revoking = true;
      this.queue({ flags: restriction, at, revocation: true });
      return;
    }
    const changed = !this.desired || !same(flags, this.desired);
    this.desired = flags;
    if (changed || (!this.token && !this.inFlight) || this.lastSentAt === null || at - this.lastSentAt >= MOTION_READINESS_HEARTBEAT_MS)
      this.queue({ flags, at });
  }

  revoke(at = this.now(), gaitAvailable = this.desired?.gaitAvailable ?? false) {
    const flags = closed(gaitAvailable);
    const changed = !this.desired || !same(flags, this.desired);
    this.desired = flags;
    // Replacing the sole candidate drops every unsent true heartbeat.
    if (changed || (!this.token && !this.inFlight) || this.lastSentAt === null ||
        at - this.lastSentAt >= MOTION_READINESS_HEARTBEAT_MS ||
        (this.inFlight && this.pending?.flags.ready)) {
      this.revoking = true;
      this.queue({ flags, at, revocation: true });
    }
  }

  dispose() {
    if (this.disposed) return;
    this.revoke();
    this.disposed = true;
    this.registration += 1;
  }

  private now() { return this.options.now?.() ?? performance.now(); }
  private acceptClock(at: number) {
    const valid = Number.isFinite(at) && at >= 0 && (this.lastNow === null || at >= this.lastNow);
    if (Number.isFinite(at) && at >= 0) this.lastNow = at;
    return valid;
  }
  private queue(candidate: Candidate) {
    if (this.inFlight) { this.pending = candidate; return; }
    this.dispatch(candidate);
  }
  private dispatch(candidate: Candidate) {
    if (!this.token) {
      if (this.disposed) return;
      this.inFlight = true;
      this.pending = candidate;
      const registration = ++this.registration;
      const complete = (token: string | null) => {
        this.inFlight = false;
        const next = this.pending;
        this.pending = null;
        if (this.disposed || registration !== this.registration) return;
        if (typeof token !== "string" || !token) {
          this.revoking = false;
          return; // Retry only on a later existing animation tick.
        }
        this.token = token;
        if (next) this.dispatch(next);
      };
      try { Promise.resolve(this.options.register(this.rendererEpoch)).then(complete, () => complete(null)); }
      catch { complete(null); }
      return;
    }
    const now = this.now();
    let flags = candidate.flags;
    let revocation = candidate.revocation === true;
    // A queued true created before a stalled response is not a fresh heartbeat.
    if (flags.ready && (this.disposed || !Number.isFinite(now) || now < candidate.at ||
        now - candidate.at > MOTION_READINESS_HEARTBEAT_MS)) {
      flags = closed(flags.gaitAvailable);
      this.desired = flags;
      this.revoking = true;
      revocation = true;
    }
    this.inFlight = true;
    this.lastSentAt = Number.isFinite(now) ? now : candidate.at;
    const payload = { rendererEpoch: this.rendererEpoch, token: this.token, sequence: (++this.sequence).toString(), ...flags };
    const complete = (accepted: boolean) => {
      this.inFlight = false;
      if (accepted !== true) {
        this.token = null;
        this.pending = null;
        this.revoking = false;
        return; // Re-register only when a later caller tick updates readiness.
      }
      if (revocation) this.revoking = false;
      const next = this.pending;
      this.pending = null;
      if (next) this.dispatch(next);
    };
    try { Promise.resolve(this.options.send(payload)).then(complete, () => complete(false)); }
    catch { complete(false); }
  }
}

/** Create only in the pet WebView. Native separately checks the window label.
 * update() belongs in CatGirl's existing 250 ms tick; this owns no interval.
 */
export function createPetMotionReadiness() {
  const owner = new PetMotionReadinessOwner({
    register: (rendererEpoch) => isDesktop ? invoke<string>("register_pet_motion_renderer", { rendererEpoch }) : Promise.resolve("browser-preview"),
    send: (readiness) => isDesktop ? invoke<boolean>("set_pet_motion_readiness", { readiness }) : Promise.resolve(true),
  });
  const reduced = typeof matchMedia === "function" ? matchMedia("(prefers-reduced-motion: reduce)") : null;
  let disposed = false;
  let latest: MotionReadinessInput = { ready: false, gaitAvailable: false, gaitReady: false };
  const update = (input: MotionReadinessInput, at?: number) => {
    if (disposed) return;
    latest = input;
    owner.update({ ...input, visible: input.visible !== false && document.visibilityState === "visible",
      reducedMotion: input.reducedMotion === true || reduced?.matches === true }, at);
  };
  const restrict = () => update(latest);
  const hide = () => owner.revoke();
  document.addEventListener("visibilitychange", restrict);
  window.addEventListener("pagehide", hide);
  reduced?.addEventListener("change", restrict);
  return {
    rendererEpoch: owner.rendererEpoch,
    update,
    dispose() {
      if (disposed) return;
      disposed = true;
      document.removeEventListener("visibilitychange", restrict);
      window.removeEventListener("pagehide", hide);
      reduced?.removeEventListener("change", restrict);
      owner.dispose();
    },
  };
}
