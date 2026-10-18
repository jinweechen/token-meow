import type {
  PanelNavigationReply, PanelNavigationRequest, PetChatContext,
  PetChatOpenRequest, PetChatOpenResult, PetDetails, PetDetailsRequest,
  PetPanelRequest, PetPulse,
} from "./contracts";
import { localDate } from "./format";

const U64_MAX = 18_446_744_073_709_551_615n;
const decimal = (value: unknown): value is string =>
  typeof value === "string" && /^(0|[1-9]\d{0,19})$/.test(value) &&
  BigInt(value) <= U64_MAX;
const object = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
function validDomain(value: unknown): value is PetDetailsRequest {
  return object(value) &&
    typeof value.day === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value.day) &&
    typeof value.runtimeEpoch === "string" && value.runtimeEpoch.length > 0 &&
    typeof value.sourceScopeRevision === "string" && value.sourceScopeRevision.length > 0;
}
function domainKey(value: PetDetailsRequest): string {
  return JSON.stringify([value.runtimeEpoch, value.sourceScopeRevision, value.day]);
}

/** Parse transport sequence/deadline only; this never supplies ledger facts. */
export function parsePanelNavigationReply(value: unknown): PanelNavigationReply {
  if (!object(value) || !decimal(value.watermark) ||
      (value.retry !== undefined && value.retry !== true))
    throw new Error("面板导航水位无效，请重新单击小卡");
  const pending = value.pending;
  if (pending !== null && (
    !object(pending) || !decimal(pending.revision) || pending.revision === "0" ||
    BigInt(pending.revision) > BigInt(value.watermark) ||
    pending.destination !== "inbox" || !validDomain(pending.petDomain) ||
    typeof pending.remainingMs !== "number" || !Number.isInteger(pending.remainingMs) ||
    pending.remainingMs <= 0 || pending.remainingMs > 10_000
  )) throw new Error("面板导航请求已过期或不可用");
  return value as unknown as PanelNavigationReply;
}
export interface PanelNavigationIntent extends PetPanelRequest {
  revision: string;
  /** Count from request send time, never reply arrival. */
  sentAt: number;
  deadline: number;
}
export interface PanelNavigationHost {
  now(): number;
  listen(wake: () => void): Promise<() => void>;
  request(request: PanelNavigationRequest): Promise<PanelNavigationReply>;
  apply(intent: PanelNavigationIntent): boolean;
  error(error: unknown): void;
}

/** One native request plus one coalesced wake. No timer, focus or ledger writes. */
export class PanelNavigationController {
  private live = true;
  private visible = false;
  private subscribed = false;
  private remove: (() => void) | null = null;
  private generation = 0;
  private watermark = 0n;
  private handled = 0n;
  private active: (PanelNavigationIntent & { generation: number }) | null = null;
  private expiry: { revision: string; sentAt: number; deadline: number } | null = null;
  private pullWanted = false;
  private cancelWanted = false;
  private ackWanted: string | null = null;
  private retryBudget = 0;
  private running: Promise<void> | null = null;

  constructor(private readonly host: PanelNavigationHost) { void this.connect(); }
  private async connect() {
    try {
      const remove = await this.host.listen(() => this.wake());
      if (!this.live) { remove(); return; }
      this.remove = remove;
      this.subscribed = true;
      this.wake();
    } catch (error) { if (this.live) this.report(error); }
  }
  private report(error: unknown) {
    try { this.host.error(error); } catch { /* Reporting cannot restart the lane. */ }
  }
  setVisible(visible: boolean) {
    if (!this.live || this.visible === visible) return;
    this.visible = visible;
    if (!visible) {
      ++this.generation;
      this.active = null;
      this.ackWanted = null;
      this.pullWanted = false;
    } else this.wake();
  }
  wake() {
    if (!this.live) return;
    this.pullWanted = true;
    this.retryBudget = 1;
    this.pump();
  }
  /** Revoke local authority synchronously, before the cancel IPC can settle. */
  cancel() {
    if (!this.live) return;
    ++this.generation;
    this.active = null;
    this.ackWanted = null;
    this.pullWanted = false;
    this.cancelWanted = true;
    this.retryBudget = 1;
    this.pump();
  }
  /** React must confirm that today's all-source unread panel really mounted. */
  commit(revision: string): boolean {
    const active = this.active, now = this.host.now();
    if (!this.live || !this.visible || !active ||
        active.revision !== revision || active.generation !== this.generation ||
        now < active.sentAt || now >= active.deadline ||
        BigInt(revision) <= this.handled) return false;
    this.handled = BigInt(revision);
    this.ackWanted = revision;
    this.pump();
    return true;
  }
  private pump() {
    if (!this.live || !this.subscribed || this.running) return;
    let complete!: () => void;
    const running = new Promise<void>((resolve) => { complete = resolve; });
    this.running = running;
    void this.drain().finally(() => {
      if (this.running === running) this.running = null;
      complete();
      if (this.live && (this.cancelWanted ||
          (this.visible && (this.ackWanted !== null || this.pullWanted)))) this.pump();
    });
  }
  private async drain() {
    while (this.live && this.subscribed) {
      let request: PanelNavigationRequest;
      if (this.cancelWanted) {
        this.cancelWanted = false;
        request = { action: "cancel" };
      } else if (this.ackWanted && this.visible) {
        const now = this.host.now();
        if (!this.active || this.active.revision !== this.ackWanted ||
            this.active.generation !== this.generation ||
            now < this.active.sentAt || now >= this.active.deadline) {
          this.ackWanted = null;
          continue;
        }
        request = { action: "ack", revision: this.ackWanted };
        this.ackWanted = null;
      } else if (this.pullWanted && this.visible) {
        this.pullWanted = false;
        request = { action: "peek" };
      } else return;
      const generation = this.generation, sentAt = this.host.now();
      try {
        const reply = parsePanelNavigationReply(await this.host.request(request));
        if (!this.live || generation !== this.generation) continue;
        if (reply.retry && this.retryBudget > 0 && this.visible) {
          --this.retryBudget;
          this.pullWanted = true;
        }
        const watermark = BigInt(reply.watermark);
        if (watermark < this.watermark) continue;
        this.watermark = watermark;
        if (!reply.pending) this.active = null;
        else if (this.visible && BigInt(reply.pending.revision) > this.handled) {
          const previous = this.expiry?.revision === reply.pending.revision ? this.expiry : null;
          this.expiry = {
            revision: reply.pending.revision,
            sentAt: previous?.sentAt ?? sentAt,
            deadline: Math.min(previous?.deadline ?? Infinity, sentAt + reply.pending.remainingMs),
          };
          const intent: PanelNavigationIntent = {
            destination: "inbox", revision: reply.pending.revision,
            petDomain: { ...reply.pending.petDomain },
            sentAt: this.expiry.sentAt, deadline: this.expiry.deadline,
          };
          const now = this.host.now();
          if (this.active?.revision === intent.revision) {
            this.active.deadline = intent.deadline;
          } else if (now >= intent.sentAt && now < intent.deadline) {
            this.active = { ...intent, generation };
            if (!this.host.apply(intent)) this.active = null;
          }
        }
      } catch (error) {
        if (this.live && generation === this.generation) this.report(error);
      }
    }
  }
  dispose() {
    this.live = false;
    ++this.generation;
    this.active = null;
    this.ackWanted = null;
    this.pullWanted = false;
    this.cancelWanted = false;
    this.expiry = null;
    this.remove?.();
    this.remove = null;
    // Native owns close/destroy cancellation; StrictMode cleanup must not
    // cancel a legitimate request that the replacement owner has not received.
  }
}

export interface PetCardSnapshot {
  ready: boolean; pulse: PetPulse | null;
  detailsReady: boolean; details: PetDetails | null;
}
interface PetCardCapture {
  petDomain: PetDetailsRequest; chat: PetChatContext | null; at: number;
}
const fresh = (asOf: string, now: number) => {
  const age = now - Date.parse(asOf);
  return Number.isFinite(age) && age >= -1000 && age <= 15_000;
};
/** Capture accepted identity only, never an inferred foreground conversation. */
export function capturePetCard(
  state: PetCardSnapshot, now: number, chatRequired = false,
): PetCardCapture | null {
  const pulse = state.pulse;
  if (!state.ready || !pulse || !validDomain(pulse) ||
      pulse.day !== localDate(new Date(now)) || !fresh(pulse.asOf, now)) return null;
  const petDomain = {
    day: pulse.day, runtimeEpoch: pulse.runtimeEpoch,
    sourceScopeRevision: pulse.sourceScopeRevision,
  };
  if (!chatRequired) return { petDomain, chat: null, at: now };
  const details = state.details, chat = details?.recentChat;
  if (!state.detailsReady || !details ||
      domainKey(details) !== domainKey(petDomain) || !fresh(details.asOf, now) ||
      !chat || typeof chat.sourceId !== "string" || !chat.sourceId ||
      typeof chat.threadId !== "string" || !chat.threadId) return null;
  return { petDomain, chat: { sourceId: chat.sourceId, threadId: chat.threadId }, at: now };
}
export interface PetCardActionHost {
  desktop: boolean;
  now(): number;
  current(): PetCardSnapshot;
  close(): void;
  interrupt(): void;
  busy(value: boolean): void;
  openInbox(request: PetPanelRequest): Promise<unknown>;
  openChat(request: PetChatOpenRequest): Promise<PetChatOpenResult>;
  copy(threadId: string): Promise<void>;
  notify(message: string): void;
  refresh(): Promise<unknown>;
}
function errorText(error: unknown): string {
  return typeof error === "string" ? error :
    error instanceof Error ? error.message : "本地操作未完成";
}
/** Own close is harmless; hidden/domain/owner changes revoke a pending click. */
export class PetCardActionController {
  private live = true;
  private visible = false;
  private generation = 0;
  private running = false;
  constructor(private readonly host: PetCardActionHost) {}
  setVisible(visible: boolean) {
    if (this.visible === visible) return;
    this.visible = visible;
    if (!visible) this.invalidate();
  }
  invalidate() { ++this.generation; }
  openInbox(): Promise<void> { return this.submit(false); }
  openChat(): Promise<void> { return this.submit(true); }
  private async submit(chatRequired: boolean) {
    if (!this.live || !this.visible || this.running) return;
    if (!this.host.desktop) {
      this.host.notify("演示小卡不打开或复制真实聊天");
      return;
    }
    const capture = capturePetCard(this.host.current(), this.host.now(), chatRequired);
    if (!capture) {
      this.host.notify(chatRequired ? "今日暂无可定位聊天，请更新小卡" : "小卡已过期，请更新后再试");
      void this.host.refresh().catch(() => undefined);
      return;
    }
    const generation = this.generation;
    const current = () => {
      const next = capturePetCard(this.host.current(), this.host.now());
      return this.live && this.visible && this.generation === generation &&
        this.host.now() >= capture.at && next !== null &&
        domainKey(next.petDomain) === domainKey(capture.petDomain);
    };
    this.running = true;
    this.host.busy(true);
    // Both identity fields were copied before close invalidates detailsReady.
    try {
      this.host.close();
      this.host.interrupt();
      if (!chatRequired) {
        await this.host.openInbox({ destination: "inbox", petDomain: capture.petDomain });
        return;
      }
      const chat = capture.chat!;
      const result = await this.host.openChat({ petDomain: capture.petDomain, chat });
      if (!current()) return;
      if (result.status === "submitted") {
        this.host.notify("已请求打开聊天");
      } else if (result.status === "copyRequired" &&
                 result.threadId === chat.threadId && typeof result.message === "string") {
        // A rejected IPC is never permission to copy an old identity.
        try {
          if (!current()) return;
          await this.host.copy(chat.threadId);
          if (current()) this.host.notify(result.message + " · 会话标识已复制");
        } catch {
          if (current()) this.host.notify(result.message + " · 会话标识：" + chat.threadId);
        }
      } else throw new Error("聊天身份回复无效，请更新小卡");
    } catch (error) {
      if (current()) {
        this.host.notify("小卡已过期或暂不可用，请更新。" + errorText(error));
        void this.host.refresh().catch(() => undefined);
      }
    } finally {
      this.running = false;
      if (this.live) this.host.busy(false);
    }
  }
  dispose() {
    this.live = false;
    this.visible = false;
    this.invalidate();
  }
}

