import type { Query } from "./contracts";
import { queryDates } from "./format";

export type DatePeriod = "1" | "7" | "30" | "custom";
export type QueryUpdate = Query | ((previous: Query) => Query);

export interface QueryState {
  query: Query;
  period: DatePeriod;
  revision: number;
}

export interface QueryRequest {
  readonly serial: number;
  readonly revision: number;
  readonly query: Readonly<Query>;
  readonly key: string;
}

/** Bind a response to every effective filter; Core snapshots have no query echo. */
export function queryKey(query: Query): string {
  return JSON.stringify([
    query.from ?? null,
    query.to ?? null,
    query.sourceId ?? null,
    query.threadId ?? null,
    query.rootTurnId ?? null,
    query.clientKind ?? null,
    query.providerId ?? null,
    query.project ?? null,
    query.model ?? null,
    query.agentKind ?? null,
    query.includeLegacy ?? false,
  ]);
}

export function queryScopeReady(
  accepted: Pick<QueryState, "query" | "revision"> | null,
  current: QueryState,
): boolean {
  return (
    accepted != null &&
    accepted.revision === current.revision &&
    queryKey(accepted.query) === queryKey(current.query)
  );
}

/** No timer: callers reconcile on the existing refresh/visibility/export paths. */
export class QueryLifecycle {
  private query: Query;
  private period: DatePeriod;
  private revision = 0;
  private serial = 0;

  constructor(search: string, now = new Date()) {
    this.period =
      new URLSearchParams(search).get("window") === "pet" ? "1" : "7";
    this.query = { ...queryDates(Number(this.period), now), includeLegacy: false };
  }

  state(): QueryState {
    return {
      query: { ...this.query },
      period: this.period,
      revision: this.revision,
    };
  }

  synchronize(now = new Date()): boolean {
    if (this.period === "custom") return false;
    return this.replace({
      ...this.query,
      ...queryDates(Number(this.period), now),
    });
  }

  update(update: QueryUpdate, now = new Date()): void {
    this.synchronize(now);
    this.replace(
      typeof update === "function" ? update({ ...this.query }) : update,
    );
    // Changing a scope never pins an automatic preset to a caller's old dates.
    this.synchronize(now);
  }

  selectPeriod(period: DatePeriod, now = new Date()): void {
    this.synchronize(now);
    this.period = period;
    this.synchronize(now);
  }

  /** The pet badge counts today's unread across every source and identity. */
  openPetInbox(day: string, now = new Date()): boolean {
    const today = queryDates(1, now);
    if (day !== today.from) return false;
    this.period = "1";
    const changed = this.replace({ ...today, includeLegacy: false });
    // Even the same date/filter is a new navigation: revoke old in-flight reads.
    if (!changed) {
      ++this.revision;
      this.invalidate();
    }
    return true;
  }

  begin(now = new Date()): QueryRequest {
    this.synchronize(now);
    const query = Object.freeze({ ...this.query });
    return Object.freeze({
      serial: ++this.serial,
      revision: this.revision,
      query,
      key: queryKey(query),
    });
  }

  isCurrent(request: QueryRequest, now = new Date()): boolean {
    // A reply may be the first callback after midnight or a clock rollback.
    this.synchronize(now);
    return (
      request.serial === this.serial &&
      request.revision === this.revision &&
      request.key === queryKey(this.query)
    );
  }

  invalidate(): void {
    ++this.serial;
  }

  /** Facts can change even when a source keeps the same ID and all filters match. */
  domainChanged(): void {
    ++this.revision;
    this.invalidate();
  }

  private replace(query: Query): boolean {
    if (queryKey(query) === queryKey(this.query)) return false;
    this.query = { ...query };
    ++this.revision;
    this.invalidate();
    return true;
  }
}
