import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isQueryDomainChange } from "./queryDomain";
import { parseMixtureStudiesOverview, type MixtureStudiesOverview, type SaveMixtureStudy, type DeleteMixtureStudy } from "./mixtureStudyContracts";

export type MixtureStudyInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Guard = () => boolean;
export interface MixtureStudyClient {
  read: (sourceId: string, current?: Guard) => Promise<MixtureStudiesOverview>;
  save: (payload: SaveMixtureStudy, current?: Guard) => Promise<MixtureStudiesOverview>;
  delete: (payload: DeleteMixtureStudy, current?: Guard) => Promise<MixtureStudiesOverview>;
  subscribe: (changed: (domainChanged: boolean) => void) => Promise<() => void>;
}
export function createMixtureStudyClient(call: MixtureStudyInvoke, subscribe: MixtureStudyClient["subscribe"]): MixtureStudyClient {
  let tail: Promise<unknown> = Promise.resolve();
  const serial = (command: string, sourceId: string, args: Record<string, unknown>, current?: Guard) => {
    const task = tail.catch(() => {}).then(async () => {
      if (current && !current()) throw Error("手动实验操作范围已变化。");
      return parseMixtureStudiesOverview(await call(command, args), sourceId);
    });
    tail = task.catch(() => {});
    return task;
  };
  const write = <T extends { sourceId: string; expectedScope: string }>(command: string, payload: T, current?: Guard) => {
    if (!payload.expectedScope || !payload.sourceId) return Promise.reject(Error("缺少当前来源的实验写入范围。"));
    return serial(command, payload.sourceId, { payload }, current);
  };
  return { read: (sourceId, guard) => serial("read_mixture_studies", sourceId, { sourceId }, guard),
    save: (p, g) => write("save_mixture_study", p, g), delete: (p, g) => write("delete_mixture_study", p, g), subscribe };
}
export const mixtureStudyClient = createMixtureStudyClient(invoke, async changed => {
  const results = await Promise.allSettled([
    listen("mixture-studies-updated", () => changed(false)),
    listen("ledger-updated", event => changed(isQueryDomainChange(event.payload))),
  ]);
  const removes = results.flatMap(result => result.status === "fulfilled" ? [result.value] : []);
  if (results.some(result => result.status === "rejected")) { removes.forEach(remove => remove()); throw Error("手动实验更新监听暂不可用。"); }
  return () => removes.forEach(remove => remove());
});

export interface MixtureStudyViewState { overview: MixtureStudiesOverview | null; pending: boolean; fresh: boolean; action: string | null; error: string | null; notice: string | null; formEpoch: number }
const empty = (): MixtureStudyViewState => ({ overview: null, pending: false, fresh: false, action: null, error: null, notice: null, formEpoch: 0 });
const failureText = (error: unknown) => error instanceof Error ? error.message : typeof error === "string" ? error : "手动实验操作暂不可用。";

/** 一个在途操作与一次合并补读；事件不携带计量事实，失效后始终重读 Core。 */
export class MixtureStudyReadModel {
  private view = empty(); private ended = false; private busy = false; private again = false;
  private epoch = 0; private readRevision = 0; private remove: (() => void) | null = null;
  private operationError: string | null = null;
  constructor(private readonly client: MixtureStudyClient, private readonly sourceId: string,
    private readonly publish: (state: MixtureStudyViewState) => void) {}
  state() { return this.view; }
  private emit(patch: Partial<MixtureStudyViewState>) { if (!this.ended) { this.view = { ...this.view, ...patch }; this.publish(this.view); } }
  start() {
    void this.client.subscribe(domain => this.invalidate(domain)).then(remove => { if (this.ended) remove(); else this.remove = remove; })
      .catch(error => { if (!this.ended) this.emit({ error: failureText(error) }); });
    this.reload();
  }
  reload() { if (!this.ended) { this.again = true; void this.read(); } }
  invalidate(domainChanged: boolean) {
    if (this.ended) return;
    this.readRevision++;
    if (domainChanged) { this.epoch++; this.operationError = null; this.emit({ overview: null, fresh: false, formEpoch: this.view.formEpoch + 1, error: null, notice: null }); }
    else this.emit({ fresh: false });
    this.reload();
  }
  private accept(value: MixtureStudiesOverview) {
    const before = this.view.overview;
    if (before?.scopeToken === value.scopeToken && before.revision > value.revision) throw Error("手动实验返回了较早修订，请更新视图。");
    this.emit({ overview: value, fresh: true, ...(before && before.scopeToken !== value.scopeToken ? { formEpoch: this.view.formEpoch + 1 } : {}) });
  }
  private async read() {
    if (this.busy || this.ended || !this.again) return;
    this.busy = true; this.again = false;
    const epoch = this.epoch, revision = this.readRevision;
    const current = () => !this.ended && this.epoch === epoch;
    this.emit({ pending: true });
    try {
      const value = await this.client.read(this.sourceId, current);
      if (current() && revision === this.readRevision) { this.accept(value); this.emit({ error: this.operationError }); }
    } catch (error) { if (current() && revision === this.readRevision) this.emit({ error: failureText(error), fresh: false }); }
    finally { this.busy = false; if (!this.ended) { this.emit({ pending: false }); if (this.again) void this.read(); } }
  }
  async mutate(name: string, expectedScope: string, operation: (current: Guard) => Promise<MixtureStudiesOverview>, notice: string): Promise<boolean> {
    if (this.ended || this.busy || !this.view.fresh || this.view.overview?.scopeToken !== expectedScope) return false;
    const epoch = this.epoch, current = () => !this.ended && this.epoch === epoch;
    this.busy = true; this.operationError = null; this.readRevision++; this.emit({ pending: true, action: name, error: null, notice: null });
    try {
      const value = await operation(current);
      if (!current()) return false;
      if (value.scopeToken !== expectedScope) { this.invalidate(true); return false; }
      this.accept(value); this.emit({ notice }); return true;
    } catch (error) { if (current()) { this.operationError = failureText(error); this.emit({ error: this.operationError, fresh: false }); this.again = true; } return false; }
    finally { this.busy = false; if (!this.ended) { this.emit({ pending: false, action: null }); if (this.again) void this.read(); } }
  }
  dispose() { this.ended = true; this.epoch++; this.readRevision++; this.again = false; this.remove?.(); this.remove = null; }
}

