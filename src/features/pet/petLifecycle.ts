import type {
  PetCursor,
  PetDetails,
  PetDetailsRequest,
  PetPulse,
  PetReadHeader,
} from "../../shared/contracts";
import { ViewRefreshLoop, type IntervalHost } from "../../shared/viewRefreshLoop";

const OBSERVATION_GAP = 15_000;
const decimal = (value: unknown): value is string =>
  typeof value === "string" && /^(0|[1-9]\d*)$/.test(value);
export function petDay(now: number): string {
  return new Date(now + 8 * 60 * 60 * 1000).toISOString().slice(0, 10);
}
export function petDomain(value: Pick<PetReadHeader, "runtimeEpoch" | "sourceScopeRevision" | "day">): string {
  return JSON.stringify([value.runtimeEpoch, value.sourceScopeRevision, value.day]);
}
function message(error: unknown): string {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "宠物数据暂时未能更新";
}
function immutablePulse(pulse: PetPulse): PetPulse {
  return Object.freeze({
    ...pulse,
    prompts: Object.freeze(pulse.prompts.map((prompt) => Object.freeze({ ...prompt }))),
    promptFeed: Object.freeze({
      ...pulse.promptFeed,
      cursor: Object.freeze({ ...pulse.promptFeed.cursor }),
      dropped: Object.freeze({ ...pulse.promptFeed.dropped }),
    }),
  });
}
export interface PetReadState {
  pulse: PetPulse | null;
  ready: boolean;
  details: PetDetails | null;
  detailsReady: boolean;
  connectionError: string | null;
  detailsError: string | null;
}
export interface PetReadHost {
  now(): number;
  pulse(cursor: PetCursor | null): Promise<PetPulse>;
  details(request: PetDetailsRequest): Promise<PetDetails>;
  intervals?: IntervalHost;
}

/** No Query/Snapshot dependency. Each read channel is single-flight and fenced. */
export class PetReadLifecycle {
  private value: PetReadState = {
    pulse: null,
    ready: false,
    details: null,
    detailsReady: false,
    connectionError: null,
    detailsError: null,
  };
  private listeners = new Set<(state: PetReadState) => void>();
  private loop: ViewRefreshLoop;
  private visible = false;
  private disposed = false;
  private generation = 0;
  private serial = 0;
  private cursor: PetCursor | null = null;
  private day: string;
  private lastClock: number;
  private acceptedAt: number | null = null;
  private detailsOpen = false;
  private detailsRunning = false;
  private detailsAgain = false;
  private detailSerial = 0;

  constructor(private host: PetReadHost) {
    this.lastClock = host.now();
    this.day = petDay(this.lastClock);
    this.loop = new ViewRefreshLoop(() => this.readPulse(), host.intervals);
  }
  state(): PetReadState {
    return this.value;
  }
  subscribe(listener: (state: PetReadState) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  private publish(update: Partial<PetReadState>) {
    if (this.disposed) return;
    this.value = { ...this.value, ...update };
    this.listeners.forEach((listener) => listener(this.value));
  }
  private fence() {
    ++this.generation;
    this.cursor = null;
    this.acceptedAt = null;
    this.invalidateDetails();
    this.publish({ ready: false, detailsReady: false });
  }
  private reconcileClock(): boolean {
    const now = this.host.now();
    const day = petDay(now);
    const changed =
      day !== this.day ||
      now < this.lastClock ||
      (this.acceptedAt !== null && now - this.acceptedAt > OBSERVATION_GAP);
    this.lastClock = now;
    this.day = day;
    if (changed) this.fence();
    return changed;
  }
  setVisible(visible: boolean) {
    if (this.disposed) return;
    const clockChanged = this.reconcileClock();
    if (this.visible === visible) {
      if (visible && clockChanged) void this.loop.request(true);
      return;
    }
    this.visible = visible;
    this.fence();
    if (!visible) this.detailsOpen = false;
    this.loop.setVisible(visible);
  }
  request(): Promise<void> {
    this.reconcileClock();
    return this.loop.request(true);
  }
  domainChanged() {
    if (this.disposed) return;
    this.fence();
    if (this.visible) void this.loop.request(true);
  }
  settingsChanged() {
    if (this.disposed) return;
    ++this.generation;
    this.invalidateDetails();
    // Settings invalidate old responses, not the persistent arrival cursor.
    this.publish({ ready: false, detailsReady: false });
    if (this.visible) void this.loop.request(true);
  }
  private headerValid(value: PetReadHeader, day: string): boolean {
    const age = this.host.now() - Date.parse(value.asOf);
    return (
      value.protocolVersion === 1 &&
      typeof value.runtimeEpoch === "string" && value.runtimeEpoch.length > 0 &&
      typeof value.sourceScopeRevision === "string" && value.sourceScopeRevision.length > 0 &&
      decimal(value.revision) && value.day === day &&
      Number.isFinite(age) && age >= -1000 && age <= OBSERVATION_GAP
    );
  }
  private async readPulse() {
    if (this.disposed || !this.visible) return;
    this.reconcileClock();
    const ticket = {
      generation: this.generation,
      serial: ++this.serial,
      day: this.day,
      cursor: this.cursor,
    };
    try {
      const incoming = await this.host.pulse(ticket.cursor);
      const clockChanged = this.reconcileClock();
      if (this.disposed || !this.visible || ticket.generation !== this.generation || ticket.serial !== this.serial) {
        if (clockChanged && this.visible) void this.loop.request(true);
        return;
      }
      const cursor = incoming.promptFeed.cursor;
      const previous = this.value.pulse;
      const sameDomain = previous !== null && petDomain(previous) === petDomain(incoming);
      if (
        !this.headerValid(incoming, ticket.day) ||
        !decimal(cursor.sequence) || petDomain(cursor) !== petDomain(incoming) ||
        incoming.prompts.length > 8 ||
        (incoming.promptFeed.baseline && incoming.prompts.length !== 0) ||
        (ticket.cursor === null && !incoming.promptFeed.baseline) ||
        (!sameDomain && previous !== null && !incoming.promptFeed.baseline) ||
        (sameDomain && BigInt(incoming.revision) <= BigInt(previous!.revision)) ||
        (ticket.cursor !== null && petDomain(ticket.cursor) === petDomain(cursor) && BigInt(cursor.sequence) < BigInt(ticket.cursor.sequence))
      ) throw new Error("宠物数据的日期或提示水位不匹配，等待重新读取");
      if (previous && !sameDomain) this.invalidateDetails();
      const pulse = immutablePulse(incoming);
      // Arrival cursor commits only after the complete current-generation reply.
      this.cursor = pulse.promptFeed.cursor;
      this.acceptedAt = this.host.now();
      this.publish({ pulse, ready: true, connectionError: null, ...(sameDomain ? {} : { detailsReady: false }) });
      if (this.detailsOpen) void this.readDetails();
    } catch (error) {
      if (!this.disposed && this.visible && ticket.generation === this.generation && ticket.serial === this.serial)
        this.publish({ ready: false, connectionError: message(error) });
    }
  }
  private invalidateDetails() {
    ++this.detailSerial;
    this.detailsAgain = false;
  }
  setDetailsOpen(open: boolean) {
    if (this.disposed || this.detailsOpen === open) return;
    this.detailsOpen = open;
    this.invalidateDetails();
    this.publish({ detailsReady: false, detailsError: null });
    if (open) void this.readDetails();
  }
  private async readDetails() {
    if (this.disposed || !this.visible || !this.detailsOpen || !this.value.ready || !this.value.pulse) return;
    if (this.detailsRunning) {
      this.detailsAgain = true;
      return;
    }
    if (this.reconcileClock()) {
      void this.loop.request(true);
      return;
    }
    this.detailsRunning = true;
    const pulse = this.value.pulse;
    const ticket = { serial: ++this.detailSerial, generation: this.generation, domain: petDomain(pulse) };
    const request: PetDetailsRequest = {
      day: pulse.day,
      runtimeEpoch: pulse.runtimeEpoch,
      sourceScopeRevision: pulse.sourceScopeRevision,
    };
    try {
      const details = await this.host.details(request);
      const changed = this.reconcileClock();
      if (changed && this.visible) void this.loop.request(true);
      if (this.disposed || !this.visible || !this.detailsOpen || ticket.serial !== this.detailSerial || ticket.generation !== this.generation || !this.value.ready || !this.value.pulse || petDomain(this.value.pulse) !== ticket.domain) return;
      const chat = details.recentChat;
      const chatValid = chat === null || (
        typeof chat === "object" && chat !== null && !Array.isArray(chat) &&
        typeof chat.sourceId === "string" && chat.sourceId.length > 0 &&
        typeof chat.threadId === "string" && chat.threadId.length > 0
      );
      if (!this.headerValid(details, request.day) || petDomain(details) !== ticket.domain || !chatValid)
        throw new Error("小卡日期或来源已变化，请等待更新");
      this.publish({ details, detailsReady: true, detailsError: null });
    } catch (error) {
      if (!this.disposed && this.visible && this.detailsOpen && ticket.serial === this.detailSerial && ticket.generation === this.generation)
        this.publish({ detailsReady: false, detailsError: message(error) });
    } finally {
      this.detailsRunning = false;
      if (this.detailsAgain) {
        this.detailsAgain = false;
        if (!this.disposed) void this.readDetails();
      }
    }
  }
  diagnostics() {
    return { ...this.loop.diagnostics(), generation: this.generation, cursor: this.cursor, detailsOpen: this.detailsOpen, detailsRunning: this.detailsRunning };
  }
  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    ++this.generation;
    this.invalidateDetails();
    this.loop.dispose();
    this.listeners.clear();
  }
}
