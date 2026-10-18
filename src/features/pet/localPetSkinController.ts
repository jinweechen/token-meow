import { parseLocalPetSkinPackage, parseLocalPetSkinState, type LocalPetSkinClient,
  type LocalPetSkinPackage, type LocalPetSkinState } from "../../shared/localPetSkin";
import { validateLocalPetSkinFrames } from "./localPetSkinRuntime";

export interface LocalPetSkinsSnapshot {
  library: LocalPetSkinState | null;
  selected: LocalPetSkinPackage | null;
  preview: LocalPetSkinPackage | null;
  phase: "loading" | "idle" | "importing" | "validating" | "selecting";
  progress: { checked: number; total: number } | null;
  previewReady: boolean;
  selectionRevision: number;
  error: string | null;
}
export type SkinValidator = typeof validateLocalPetSkinFrames;
const message = (error: unknown) => error instanceof Error ? error.message : "本地皮肤操作失败";

/** Window-local state. Import and Preview never write the persistent selection. */
export class LocalPetSkinController {
  private value: LocalPetSkinsSnapshot = { library: null, selected: null, preview: null,
    phase: "loading", progress: null, previewReady: false, selectionRevision: 0, error: null };
  private listeners = new Set<() => void>();
  private live = true;
  private epoch = 0;
  private reads = 0;
  private busy = false;
  private validation: AbortController | null = null;
  private unlisten: (() => void) | null = null;
  constructor(private client: LocalPetSkinClient, private validate: SkinValidator = validateLocalPetSkinFrames) {}
  snapshot = () => this.value;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(patch: Partial<LocalPetSkinsSnapshot>) {
    if (!this.live) return;
    this.value = { ...this.value, ...patch };
    for (const listener of this.listeners) listener();
  }
  private apply(input: LocalPetSkinState) {
    const library = parseLocalPetSkinState(input);
    this.update({ library, selected: library.skins.find((skin) => skin.skinId === library.selectedSkinId) ?? null });
  }
  async start() {
    void this.refresh();
    if (!this.client.subscribe) return;
    try {
      const stop = await this.client.subscribe((event) => {
        if (event?.kind === "selected") this.update({ selectionRevision: this.value.selectionRevision + 1 });
        void this.refresh();
      });
      if (!this.live) stop(); else this.unlisten = stop;
    } catch (error) { this.update({ error: message(error) }); }
  }
  refresh = async () => {
    const read = ++this.reads;
    try {
      const state = await this.client.getState();
      if (!this.live || read !== this.reads) return;
      this.apply(state);
      if (!this.busy && this.value.phase === "loading") this.update({ phase: "idle", error: null });
    } catch (error) {
      if (read === this.reads) this.update({ error: message(error), ...(this.busy ? {} : { phase: "idle" as const }) });
    }
  };
  private async inspect(pkg: LocalPetSkinPackage, epoch: number) {
    this.validation?.abort();
    const validation = new AbortController();
    this.validation = validation;
    this.update({ preview: pkg, previewReady: false, phase: "validating", progress: { checked: 0, total: pkg.manifest.frames.length }, error: null });
    try {
      await this.validate(pkg, this.client, validation.signal, (checked, total) => {
        if (this.live && epoch === this.epoch) this.update({ progress: { checked, total } });
      });
      if (this.live && epoch === this.epoch) this.update({ previewReady: true, phase: "idle", progress: null });
    } catch (error) {
      if (this.live && epoch === this.epoch) this.update({ previewReady: false, phase: "idle", progress: null, error: message(error) });
    }
  }
  importSkin = async () => {
    if (this.busy || !this.live) return;
    this.busy = true;
    const epoch = ++this.epoch;
    this.validation?.abort();
    this.update({ phase: "importing", error: null });
    try {
      const input = await this.client.chooseImport();
      if (!this.live || epoch !== this.epoch) return;
      if (input === null) { this.update({ phase: "idle" }); return; }
      const pkg = parseLocalPetSkinPackage(input);
      await this.refresh();
      await this.inspect(pkg, epoch);
    } catch (error) {
      if (epoch === this.epoch) this.update({ phase: "idle", error: message(error) });
    } finally { this.busy = false; }
  };
  previewSkin = async (skinId: string) => {
    if (this.busy || !this.live) return;
    const pkg = this.value.library?.skins.find((skin) => skin.skinId === skinId);
    if (!pkg) return;
    this.busy = true;
    try { await this.inspect(pkg, ++this.epoch); } finally { this.busy = false; }
  };
  private async select(skinId: string | null) {
    if (this.busy || !this.live) return false;
    if (skinId !== null && (!this.value.previewReady || this.value.preview?.skinId !== skinId)) return false;
    this.busy = true;
    ++this.reads;
    this.update({ phase: "selecting", error: null });
    try {
      const result = await this.client.select(skinId);
      if (!this.live) return false;
      if (result.selectedSkinId !== skinId) throw Error("本地皮肤选择响应不一致");
      ++this.reads;
      this.apply(result);
      this.update({ phase: "idle", selectionRevision: this.value.selectionRevision + 1 });
      return true;
    } catch (error) { this.update({ phase: "idle", error: message(error) }); return false; }
    finally { this.busy = false; }
  }
  useSkin = async (skinId = this.value.preview?.skinId) => skinId ? this.select(skinId) : false;
  useBuiltin = async () => this.select(null);
  removeSkin = async (skinId: string) => {
    if (this.busy || !this.live || this.value.library?.selectedSkinId === skinId) return false;
    const known = this.value.library?.skins.some((skin) => skin.skinId === skinId) ||
      this.value.library?.problems?.some((problem) => problem.skinId === skinId);
    if (!known) return false;
    this.busy = true; ++this.reads;
    this.update({ phase: "selecting", error: null });
    try {
      const result = await this.client.remove(skinId);
      if (!this.live) return false;
      ++this.reads; ++this.epoch;
      this.apply(result);
      if (this.value.preview?.skinId === skinId) {
        this.validation?.abort();
        this.update({ preview: null, previewReady: false, progress: null });
      }
      this.update({ phase: "idle" });
      return true;
    } catch (error) { this.update({ phase: "idle", error: message(error) }); return false; }
    finally { this.busy = false; }
  };
  dispose() {
    this.live = false; ++this.epoch; ++this.reads;
    this.validation?.abort(); this.unlisten?.(); this.listeners.clear();
  }
}
