import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AccountSnapshot, RateWindow, SourceInstance } from "./contracts";
import { accountExpired } from "./format";

export type SourceAccountStatus = "not_read" | "ready" | "stale" | "unsupported" | "disabled" | "unavailable";
export interface SourceAccountRow {
  sourceId: string; clientKind: string; primary: boolean; enabled: boolean; bindingGeneration: string;
  status: SourceAccountStatus; identityStatus: "unproven" | "unknown";
  snapshot: AccountSnapshot | null; lastAttemptAt: string | null; reasons: string[];
}
export interface SourceAccountsOverview {
  asOf: string; sourceCount: number; confirmedAccountCount: null; rows: SourceAccountRow[];
}
type Json = Record<string, unknown>;
const object = (value: unknown): value is Json => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown, max = 512): value is string => typeof value === "string" && value.length > 0 && value.length <= max;
const decimal = (value: unknown): value is string => typeof value === "string" && /^(0|[1-9]\d{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
const date = (value: unknown): string | null => typeof value === "string" && value.length <= 64 && Number.isFinite(Date.parse(value)) ? value : null;
const number = (value: unknown, max = Number.MAX_SAFE_INTEGER): number | null => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= max ? value : null;
function windowValue(value: unknown): RateWindow | null {
  if (!object(value)) return null;
  const duration = number(value.windowDurationMins);
  return { usedPercent: number(value.usedPercent, 100), windowDurationMins: duration && duration > 0 ? duration : null,
    resetsAt: number(value.resetsAt, 8_640_000_000_000) };
}
function accountValue(value: unknown, sourceId: string): AccountSnapshot | null {
  if (value === null) return null;
  if (!object(value) || !["unavailable", "connected", "stale", "loading"].includes(String(value.status))
      || (value.sourceId !== undefined && value.sourceId !== sourceId) || !Array.isArray(value.buckets) || value.buckets.length > 256)
    throw Error("来源账户快照格式无效");
  const ids = new Set<string>();
  const buckets = value.buckets.map(bucket => {
    if (!object(bucket) || !text(bucket.id) || ids.has(bucket.id)) throw Error("来源额度窗口标识无效");
    ids.add(bucket.id);
    return { id: bucket.id, name: text(bucket.name) ? bucket.name : bucket.id,
      ...(bucket.primary !== undefined ? { primary: windowValue(bucket.primary) } : {}),
      ...(bucket.secondary !== undefined ? { secondary: windowValue(bucket.secondary) } : {}) };
  });
  // 只投影本视图需要的白名单字段，不把身份字符串或其它账户元数据拼成账号名称。
  return { sourceId, status: value.status as AccountSnapshot["status"], collectedAt: date(value.collectedAt),
    lastAttemptAt: date(value.lastAttemptAt), error: text(value.error, 2048) ? value.error : null, buckets };
}
export function parseSourceAccountsOverview(value: unknown): SourceAccountsOverview {
  if (!object(value) || !date(value.asOf) || !Number.isSafeInteger(value.sourceCount) || Number(value.sourceCount) < 0
      || Number(value.sourceCount) > 10_000 || value.confirmedAccountCount !== null || !Array.isArray(value.rows)
      || value.rows.length !== value.sourceCount) throw Error("来源账户概览格式无效");
  const ids = new Set<string>();
  const rows = value.rows.map(row => {
    if (!object(row) || !text(row.sourceId, 256) || ids.has(row.sourceId) || !text(row.clientKind, 64)
        || typeof row.primary !== "boolean" || typeof row.enabled !== "boolean" || !decimal(row.bindingGeneration)
        || !["not_read", "ready", "stale", "unsupported", "disabled", "unavailable"].includes(String(row.status))
        || !["unproven", "unknown"].includes(String(row.identityStatus)) || !Array.isArray(row.reasons)
        || row.reasons.length > 64 || !row.reasons.every(reason => text(reason, 2048))) throw Error("来源账户行格式无效");
    ids.add(row.sourceId);
    return { sourceId: row.sourceId, clientKind: row.clientKind, primary: row.primary, enabled: row.enabled,
      bindingGeneration: row.bindingGeneration, status: row.status as SourceAccountStatus,
      identityStatus: row.identityStatus as SourceAccountRow["identityStatus"], snapshot: accountValue(row.snapshot, row.sourceId),
      lastAttemptAt: date(row.lastAttemptAt), reasons: row.reasons as string[] };
  });
  return { asOf: value.asOf as string, sourceCount: value.sourceCount as number, confirmedAccountCount: null, rows };
}
export function sourceAccountStatus(row: SourceAccountRow, now = Date.now()): SourceAccountStatus {
  if (!row.enabled) return "disabled";
  if (row.clientKind !== "codex") return "unsupported";
  if (row.status === "unavailable" && row.snapshot?.buckets.length) return "stale";
  if (row.status !== "ready") return row.status;
  if (!row.snapshot || row.snapshot.status === "loading") return "not_read";
  if (accountExpired(row.snapshot, now)) return "stale";
  return row.snapshot.status === "unavailable" ? row.snapshot.buckets.length ? "stale" : "unavailable" : "ready";
}
export interface SourceAccountsClient {
  read: (current?: () => boolean) => Promise<SourceAccountsOverview>;
  refresh: (sourceId: string, current?: () => boolean) => Promise<SourceAccountsOverview>;
  subscribe: (changed: () => void) => Promise<() => void>;
}
type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
export function createSourceAccountsClient(call: Invoke, subscribe: SourceAccountsClient["subscribe"]): SourceAccountsClient {
  let tail: Promise<unknown> = Promise.resolve();
  const serial = (command: string, args: Record<string, unknown> | undefined, current?: () => boolean) => {
    const result = tail.catch(() => {}).then(async () => {
      if (current && !current()) throw Error("来源账户读取范围已变化");
      return parseSourceAccountsOverview(await call(command, args));
    });
    tail = result; return result;
  };
  return { read: current => serial("read_source_accounts", undefined, current),
    refresh: (sourceId, current) => serial("refresh_source_account", { sourceId }, current), subscribe };
}
export const sourceAccountsClient = createSourceAccountsClient(invoke, async changed => {
  const results = await Promise.allSettled(["source-accounts-updated", "account-updated", "ledger-updated"].map(event => listen(event, changed)));
  const removes = results.flatMap(result => result.status === "fulfilled" ? [result.value] : []);
  if (results.some(result => result.status === "rejected")) { removes.forEach(remove => remove()); throw Error("来源账户更新监听暂不可用"); }
  return () => removes.forEach(remove => remove());
});

export interface SourceAccountsView {
  overview: SourceAccountsOverview | null; pending: boolean; refreshingSourceId: string | null;
  error: string | null; rowErrors: Record<string, string>;
}
const emptyView = (): SourceAccountsView => ({ overview: null, pending: false, refreshingSourceId: null, error: null, rowErrors: Object.create(null) });
const errorText = (error: unknown) => typeof error === "string" ? error : error instanceof Error ? error.message : "来源账户状态暂不可用";

/** 单条在途请求和一次合并补读；事件只使快照失效，不使用事件载荷制造账户事实。 */
export class SourceAccountsReadModel {
  private view = emptyView();
  private revision = 0;
  private busy = false;
  private again = false;
  private ended = false;
  private remove: (() => void) | null = null;
  private bindings = new Map<string, bigint>();
  private failures = new Map<string, { binding: string; at: string | null; error: string }>();
  constructor(private readonly client: SourceAccountsClient, private readonly sources: readonly SourceInstance[],
    private readonly publish: (view: SourceAccountsView) => void) {}
  start() {
    if (this.ended) return;
    void this.client.subscribe(() => this.invalidate()).then(remove => { if (this.ended) remove(); else this.remove = remove; })
      .catch(cause => { if (!this.ended) this.emit({ error: errorText(cause) }); });
    this.reload();
  }
  state() { return this.view; }
  reload() { if (!this.ended) { this.again = true; void this.read(); } }
  invalidate() {
    if (this.ended) return;
    this.revision++; this.emit({ overview: null, error: null, rowErrors: {} }); this.reload();
  }
  async refresh(sourceId: string): Promise<boolean> {
    if (this.ended || this.busy) return false;
    const source = this.sources.find(source => source.id === sourceId);
    const row = this.view.overview?.rows.find(row => row.sourceId === sourceId);
    if (!source?.enabled || source.clientKind !== "codex" || !row || !row.enabled || row.clientKind !== "codex"
        || row.status === "unsupported" || row.status === "disabled") return false;
    const revision = this.revision;
    const current = () => !this.ended && this.revision === revision;
    this.busy = true; this.emit({ pending: true, refreshingSourceId: sourceId, error: null });
    let accepted = false;
    try { const value = await this.client.refresh(sourceId, current); if (current()) { this.accept(value); accepted = true; } }
    catch (cause) {
      if (current()) {
        this.failures.set(sourceId, { binding: row.bindingGeneration, at: row.snapshot?.collectedAt ?? null, error: errorText(cause) });
        if (this.view.overview) this.accept(this.view.overview);
      }
    } finally {
      this.busy = false;
      if (!this.ended) { this.emit({ pending: false, refreshingSourceId: null }); if (this.again) void this.read(); }
    }
    return accepted;
  }
  dispose() { if (!this.ended) { this.ended = true; this.revision++; this.again = false; this.remove?.(); this.remove = null; } }
  private emit(patch: Partial<SourceAccountsView>) { this.view = { ...this.view, ...patch }; if (!this.ended) this.publish(this.view); }
  private accept(value: SourceAccountsOverview) {
    const rows = value.rows.filter(row => this.sources.some(source => source.id === row.sourceId && source.clientKind === row.clientKind && source.enabled === row.enabled));
    for (const row of rows) {
      const generation = BigInt(row.bindingGeneration), previous = this.bindings.get(row.sourceId);
      if (previous !== undefined && generation < previous) throw Error("来源绑定快照已过时，请重新读取");
    }
    const rowErrors: Record<string, string> = Object.create(null);
    const observed = rows.map(row => {
      this.bindings.set(row.sourceId, BigInt(row.bindingGeneration));
      const failure = this.failures.get(row.sourceId);
      if (!failure) return row;
      if (failure.binding !== row.bindingGeneration || failure.at !== (row.snapshot?.collectedAt ?? null)) {
        this.failures.delete(row.sourceId); return row;
      }
      rowErrors[row.sourceId] = failure.error;
      return { ...row, status: row.snapshot ? "stale" as const : "unavailable" as const,
        snapshot: row.snapshot ? { ...row.snapshot, status: "stale" as const } : null };
    });
    this.emit({ overview: { ...value, rows: observed }, error: null, rowErrors });
  }
  private async read() {
    if (this.ended || this.busy || !this.again) return;
    this.again = false; this.busy = true;
    const revision = this.revision, current = () => !this.ended && this.revision === revision;
    this.emit({ pending: true, error: null });
    try { const value = await this.client.read(current); if (current()) this.accept(value); }
    catch (cause) { if (current()) this.emit({ overview: null, error: errorText(cause) }); }
    finally { this.busy = false; if (!this.ended) { this.emit({ pending: false }); if (this.again) void this.read(); } }
  }
}
