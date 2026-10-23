export type PetPhysicsKind = "dock" | "climb" | "throw";
export type PetPhysicsRendererKind = "builtin" | "png" | "vrm";
export interface PetPhysicsContact {
  viewportWidth: number; viewportHeight: number;
  leftInset: number; rightInset: number; bottomInset: number;
}
export interface PetPhysicsRequest {
  generation: string; kind: PetPhysicsKind; phase: string;
  side?: "left" | "right"; direction?: "up" | "down"; remainingMs: number;
}
export interface PetPhysicsState {
  revision: string; rendererEpoch: string | null;
  throwEnabled: boolean; request: PetPhysicsRequest | null;
  status: "idle" | "waitingForFrame" | "moving" | "blocked"; reason: string | null;
}
export const EMPTY_PET_PHYSICS: PetPhysicsState = { revision: "0", rendererEpoch: null, throwEnabled: false, request: null, status: "idle", reason: null };
export interface PetPhysicsPaintTicket {
  rendererEpoch: string; resourceKey: string; generation: string; kind: PetPhysicsKind;
  side?: "left" | "right"; direction?: "up" | "down";
}
export interface PetPhysicsPaint {
  contact: PetPhysicsContact; ticket: PetPhysicsPaintTicket | null; climbReady: boolean;
}
export interface PetPhysicsRenderBinding {
  rendererEpoch: string; resourceKey: string; ticket: PetPhysicsPaintTicket | null;
  holdStill: boolean;
  onPainted: (paint: PetPhysicsPaint) => void;
  onUnavailable: () => void;
}
export interface PetPhysicsCredentials { rendererEpoch: string; token: string }
export interface PetPhysicsReadiness extends PetPhysicsCredentials {
  sequence: string; ready: boolean; climbReady: boolean; contact: PetPhysicsContact;
  ack?: { generation: string; kind: PetPhysicsKind };
}
type ReadinessFlags = Omit<PetPhysicsReadiness, keyof PetPhysicsCredentials | "sequence">;
const decimal = (value: unknown): value is string => typeof value === "string" && /^(0|[1-9]\d{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
export function validPetPhysicsContact(value: PetPhysicsContact): boolean {
  return [value.viewportWidth, value.viewportHeight, value.leftInset, value.rightInset, value.bottomInset].every(Number.isFinite)
    && value.viewportWidth >= 1 && value.viewportWidth <= 8192 && value.viewportHeight >= 1 && value.viewportHeight <= 8192
    && value.leftInset >= 0 && value.rightInset >= 0 && value.bottomInset >= 0
    && value.leftInset + value.rightInset < value.viewportWidth && value.bottomInset < value.viewportHeight;
}
export function samePetPhysicsTicket(a: PetPhysicsPaintTicket | null, b: PetPhysicsPaintTicket | null): boolean {
  return a === b || (!!a && !!b && a.rendererEpoch === b.rendererEpoch && a.resourceKey === b.resourceKey
    && a.generation === b.generation && a.kind === b.kind && a.side === b.side && a.direction === b.direction);
}
export function parsePetPhysicsState(value: unknown): PetPhysicsState | null {
  if (!value || typeof value !== "object") return null;
  const state = value as PetPhysicsState;
  if (!decimal(state.revision) || (state.rendererEpoch !== null && typeof state.rendererEpoch !== "string")
      || typeof state.throwEnabled !== "boolean" || !["idle", "waitingForFrame", "moving", "blocked"].includes(state.status)
      || (state.reason !== null && (typeof state.reason !== "string" || state.reason.length > 512))) return null;
  const request = state.request;
  if (request !== null && (!request || !decimal(request.generation) || !["dock", "climb", "throw"].includes(request.kind)
      || typeof request.phase !== "string" || request.phase.length > 64 || !Number.isFinite(request.remainingMs)
      || request.remainingMs < 0 || request.remainingMs > 8000
      || (request.side !== undefined && request.side !== "left" && request.side !== "right")
      || (request.direction !== undefined && request.direction !== "up" && request.direction !== "down"))) return null;
  return state;
}

/** 单条在途心跳和单条候选；撤权不能被迟到的可用帧覆盖。原生租约独立计时。 */
export class PetPhysicsReadinessOwner {
  private token: string | null = null;
  private sequence = 0n;
  private inFlight = false;
  private disposed = false;
  private revoking = false;
  private lastAt: number | null = null;
  private lastSent = -Infinity;
  private desired: ReadinessFlags | null = null;
  private pending: { value: ReadinessFlags; at: number } | null = null;
  private confirmations = new Set<{ key: string; finish: (accepted: boolean) => void }>();
  constructor(readonly rendererEpoch: string, private readonly options: {
    register: () => Promise<PetPhysicsCredentials>;
    send: (value: PetPhysicsReadiness) => Promise<boolean>;
    now?: () => number;
  }) {}
  credentials(): PetPhysicsCredentials | null { return this.token && !this.disposed ? { rendererEpoch: this.rendererEpoch, token: this.token } : null; }
  waitingForRevocation(): boolean { return this.revoking; }
  update(value: ReadinessFlags, at = this.now(), heartbeat = false) {
    if (this.disposed) return;
    const clockValid = Number.isFinite(at) && at >= 0 && (this.lastAt === null || at >= this.lastAt);
    if (Number.isFinite(at) && at >= 0) this.lastAt = at;
    if (!clockValid || !validPetPhysicsContact(value.contact) || !value.ready) { this.revoke(); return; }
    if (this.revoking) return;
    const changed = JSON.stringify(this.desired) !== JSON.stringify(value);
    this.desired = value;
    if (changed || heartbeat || !this.token || at - this.lastSent >= 500) this.queue(value, at);
  }
  revoke() {
    this.finishConfirmations(false);
    if (!this.desired) return;
    const value = { ready: false, climbReady: false, contact: this.desired.contact };
    const changed = this.desired.ready;
    this.desired = value;
    if (changed || this.pending?.value.ready) { this.revoking = true; this.queue(value, this.now()); }
  }
  dispose() { if (!this.disposed) { this.revoke(); this.disposed = true; } }
  confirm(value: ReadinessFlags): Promise<boolean> {
    if (this.disposed || this.revoking || !value.ready || !validPetPhysicsContact(value.contact)) return Promise.resolve(false);
    return new Promise(resolve => {
      const key = JSON.stringify(value);
      const entry = { key, finish: (accepted: boolean) => { clearTimeout(timeout); this.confirmations.delete(entry); resolve(accepted); } };
      const timeout = setTimeout(() => entry.finish(false), 1000);
      this.confirmations.add(entry); this.desired = value; this.queue(value, this.now());
    });
  }
  private finishConfirmations(accepted: boolean, key?: string) {
    for (const entry of Array.from(this.confirmations)) if (!key || entry.key === key) entry.finish(accepted);
  }
  private now() { return this.options.now?.() ?? performance.now(); }
  private queue(value: ReadinessFlags, at: number) {
    if (this.inFlight) { this.pending = { value, at }; return; }
    this.dispatch(value, at);
  }
  private dispatch(value: ReadinessFlags, at: number) {
    if (!this.token) {
      if (this.disposed) return;
      this.inFlight = true; this.pending = { value, at };
      Promise.resolve().then(this.options.register).then(credentials => {
        if (credentials.rendererEpoch !== this.rendererEpoch || !credentials.token) throw Error("桌宠动作登记无效");
        this.token = credentials.token;
      }).catch(() => { this.token = null; }).finally(() => {
        this.inFlight = false;
        const next = this.pending; this.pending = null;
        // 已注销但登记刚完成时，仍用原 token 撤权；不能留下刚获得的租约。
        if (this.token && next) this.dispatch(this.disposed ? { ready: false, climbReady: false, contact: next.value.contact } : next.value, next.at);
        else { this.revoking = false; this.finishConfirmations(false); }
      });
      return;
    }
    const now = this.now();
    if (value.ready && (this.disposed || !Number.isFinite(now) || now < at || now - at > 500)) {
      value = { ready: false, climbReady: false, contact: value.contact }; this.revoking = true; this.desired = value;
    }
    this.inFlight = true; this.lastSent = now;
    const payload = { ...value, rendererEpoch: this.rendererEpoch, token: this.token, sequence: (++this.sequence).toString() };
    Promise.resolve().then(() => this.options.send(payload)).then(accepted => {
      if (accepted !== true) { this.token = null; this.pending = null; this.finishConfirmations(false); }
      else this.finishConfirmations(true, JSON.stringify(value));
    }).catch(() => { this.token = null; this.pending = null; this.finishConfirmations(false); }).finally(() => {
      this.inFlight = false;
      if (!value.ready || !this.token) this.revoking = false;
      const next = this.pending; this.pending = null;
      if (next && this.token) this.dispatch(next.value, next.at);
    });
  }
}

/** 对已加载且正在显示的图片按可见裁剪区扫描透明边缘，不把透明留白当作脚底。 */
export function imagePetPhysicsContact(image: HTMLImageElement): PetPhysicsContact | null {
  if (!image.complete || image.naturalWidth < 1 || !image.parentElement) return null;
  const vw = window.innerWidth, vh = window.innerHeight;
  if (vw < 1 || vh < 1 || vw > 8192 || vh > 8192) return null;
  const imageBox = image.getBoundingClientRect(), clip = image.parentElement.getBoundingClientRect();
  if (imageBox.width < 1 || imageBox.height < 1 || clip.width < 1 || clip.height < 1) return null;
  const scale = Math.min(1, 384 / vw, 512 / vh), width = Math.ceil(vw * scale), height = Math.ceil(vh * scale);
  const canvas = document.createElement("canvas"); canvas.width = width; canvas.height = height;
  try {
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) return null;
    context.beginPath(); context.rect(clip.left * scale, clip.top * scale, clip.width * scale, clip.height * scale); context.clip();
    context.drawImage(image, imageBox.left * scale, imageBox.top * scale, imageBox.width * scale, imageBox.height * scale);
    const data = context.getImageData(0, 0, width, height).data;
    let left = width, right = -1, bottom = -1;
    for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) if (data[(y * width + x) * 4 + 3] > 0) {
      left = Math.min(left, x); right = Math.max(right, x); bottom = Math.max(bottom, y);
    }
    if (right < left) return null;
    const contact = { viewportWidth: vw, viewportHeight: vh, leftInset: Math.max(0, left / scale),
      rightInset: Math.max(0, vw - Math.min(vw, (right + 1) / scale)), bottomInset: Math.max(0, vh - Math.min(vh, (bottom + 1) / scale)) };
    return validPetPhysicsContact(contact) ? contact : null;
  } catch { return null; }
  finally { canvas.width = canvas.height = 0; }
}
