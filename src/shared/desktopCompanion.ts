/** Desktop context is presentation state, never a ledger/account fact. */
export interface CompanionCapabilities {
  systemIdle: boolean;
  roaming: boolean;
  workApps: boolean;
  launchers: boolean;
  fileDrop: boolean;
  windowEdges: boolean;
}
export interface DesktopCompanionState {
  runtimeEpoch: string;
  revision: string;
  phase: "paused" | "idle" | "playing" | "moving";
  pauseReason: string | null;
  inputIdleMs: number | null;
  activeApp: { id: string; label: string } | null;
  motionGeneration: string;
  /** A finite native segment; ordinary state revisions never renew its lease. */
  motion: { direction: "left" | "right"; remainingMs: number } | null;
  capabilities: CompanionCapabilities;
}
export interface CompanionLauncher {
  id: string;
  label: string;
  workAppEnabled: boolean;
}
const decimal = (value: unknown): value is string =>
  typeof value === "string" && /^(0|[1-9]\d{0,19})$/.test(value);
export function validCompanionState(value: unknown): value is DesktopCompanionState {
  if (!value || typeof value !== "object") return false;
  const state = value as Partial<DesktopCompanionState>;
  const capabilities = state.capabilities;
  if (!capabilities || typeof capabilities !== "object" ||
      !["systemIdle", "roaming", "workApps", "launchers", "fileDrop", "windowEdges"]
        .every((key) => typeof capabilities[key as keyof CompanionCapabilities] === "boolean")) return false;
  const motion = state.motion;
  if (state.phase === "moving") {
    if (!capabilities.roaming || !motion ||
        (motion.direction !== "left" && motion.direction !== "right") ||
        !Number.isSafeInteger(motion.remainingMs) || motion.remainingMs <= 0 || motion.remainingMs > 2400) return false;
  } else if (motion !== null) return false;
  return typeof state.runtimeEpoch === "string" && state.runtimeEpoch.length > 0 &&
    decimal(state.revision) && decimal(state.motionGeneration) &&
    typeof state.phase === "string" && ["paused", "idle", "playing", "moving"].includes(state.phase) &&
    (state.pauseReason === null || typeof state.pauseReason === "string") &&
    (state.inputIdleMs === null || (typeof state.inputIdleMs === "number" && Number.isSafeInteger(state.inputIdleMs) && state.inputIdleMs >= 0)) &&
    (state.activeApp === null || (!!state.activeApp && typeof state.activeApp.id === "string" && typeof state.activeApp.label === "string"));
}
/** Subscribe first, fetch second. Only an authoritative fetch establishes an epoch. */
export class CompanionStateReader {
  private current: DesktopCompanionState | null = null;
  private pending = new Map<string, DesktopCompanionState>();
  state() { return this.current; }
  event(value: DesktopCompanionState): "accepted" | "ignored" | "refresh" {
    if (!validCompanionState(value)) return "ignored";
    if (!this.current || value.runtimeEpoch !== this.current.runtimeEpoch) {
      const previous = this.pending.get(value.runtimeEpoch);
      if (!previous || BigInt(value.revision) > BigInt(previous.revision))
        this.pending.set(value.runtimeEpoch, value);
      if (this.pending.size > 4) this.pending.delete(this.pending.keys().next().value!);
      return "refresh";
    }
    return this.accept(value) ? "accepted" : "ignored";
  }
  snapshot(value: DesktopCompanionState): boolean {
    if (!validCompanionState(value)) return false;
    const pending = this.pending.get(value.runtimeEpoch);
    this.pending.clear();
    const incoming = pending && BigInt(pending.revision) > BigInt(value.revision) ? pending : value;
    if (this.current?.runtimeEpoch !== incoming.runtimeEpoch) {
      this.current = incoming;
      return true;
    }
    return this.accept(incoming);
  }
  private accept(value: DesktopCompanionState) {
    if (this.current && BigInt(value.revision) <= BigInt(this.current.revision)) return false;
    this.current = value;
    return true;
  }
}
const pauseLabels: Record<string, string> = {
  disabled: "空闲活动已关闭", userActive: "你在操作，小柚安静陪伴", interaction: "等你操作完再玩",
  fullscreen: "全屏时安静陪伴", locked: "锁屏时休息", doNotDisturb: "免打扰中", hidden: "角色已隐藏",
  reducedMotion: "减少动画中", inputUnavailable: "暂时无法判断空闲", unsupported: "当前环境暂不支持",
  desktopPaused: "系统暂停时安静休息", systemBusy: "系统当前不宜打扰，暂时休息",
  workActive: "先陪你完成工作", geometryUnavailable: "等待可用桌面区域", animationUnavailable: "当前形象暂未提供走动动画",
};
export function companionText(state: DesktopCompanionState | null, privacyMode = false): string {
  if (!state) return "桌面活动暂未连接";
  if (state.phase === "playing") return "空闲玩耍中";
  if (state.phase === "moving") return "桌面活动中";
  if (state.activeApp && (state.phase === "idle" || state.pauseReason === "userActive"))
    return privacyMode ? "正在陪你工作" : `陪你使用 ${state.activeApp.label}`;
  if (state.phase === "paused") return pauseLabels[state.pauseReason ?? ""] ?? "桌面活动已暂停";
  if (state.pauseReason === "disabled") return pauseLabels.disabled;
  return "陪你工作，空闲 2 分钟后再玩";
}
