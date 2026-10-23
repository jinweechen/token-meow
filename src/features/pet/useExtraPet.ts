import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { PetCursor, PetPresentation, PetPulse } from "../../shared/contracts";
import { parseExtraPet, type ExtraPet } from "../../shared/extraPets";
import { parseLocalPetSkinPackage, type LocalPetSkinPackage } from "../../shared/localPetSkin";
import { parseVrmModel, type VrmSkinModel } from "../../shared/vrmSkin";
import { PetReadLifecycle, type PetReadState } from "./petLifecycle";

export interface ExtraPetContext { pet: ExtraPet; nativeVisible: boolean; paused: boolean; png: LocalPetSkinPackage | null; vrm: VrmSkinModel | null }
export interface ExtraPetRenderer { petId: string; instanceEpoch: string; rendererEpoch: string; token: string; generation: string }
export interface ExtraPetDrag { clientX: number; clientY: number; viewportWidth: number; viewportHeight: number }
export interface ExtraPetWindowClient {
  context(): Promise<unknown>;
  register(rendererEpoch: string): Promise<unknown>;
  unregister(rendererEpoch: string, token: string): Promise<unknown>;
  pulse(cursor: PetCursor | null): Promise<PetPulse>;
  drag(request: ExtraPetDrag, renderer: ExtraPetRenderer): Promise<unknown>;
  close(renderer: ExtraPetRenderer): Promise<unknown>;
  readFrame(skinId: string, frameId: string): Promise<ArrayBuffer>;
  readModel(modelId: string): Promise<ArrayBuffer>;
  subscribe(name: string, callback: (payload: unknown) => void): Promise<() => void>;
}
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const text = (error: unknown) => typeof error === "string" ? error : error instanceof Error ? error.message : "辅助桌宠暂不可用，请在主面板重新打开。";
function bytes(value: ArrayBuffer | number[], maximum: number): ArrayBuffer {
  if (!(value instanceof ArrayBuffer) && (!Array.isArray(value) || !value.every(byte => Number.isInteger(byte) && byte >= 0 && byte <= 255))) throw Error("角色资源返回格式无效。");
  const result = value instanceof ArrayBuffer ? value : new Uint8Array(value).buffer;
  if (!result.byteLength || result.byteLength > maximum) throw Error("角色资源大小无效。");
  return result;
}
export const extraPetWindowClient: ExtraPetWindowClient = {
  context: () => invoke("extra_pet_context"),
  register: rendererEpoch => invoke("register_extra_pet_renderer", { rendererEpoch }),
  unregister: (rendererEpoch, token) => invoke("unregister_extra_pet_renderer", { rendererEpoch, token }),
  pulse: cursor => invoke("extra_pet_pulse", { cursor }),
  drag: (request, renderer) => invoke("start_extra_pet_drag", { request, rendererEpoch: renderer.rendererEpoch, token: renderer.token }),
  close: renderer => invoke("close_extra_pet", { rendererEpoch: renderer.rendererEpoch, token: renderer.token }),
  readFrame: async (skinId, frameId) => bytes(await invoke("read_pet_skin_frame", { skinId, frameId }), 4 * 1024 * 1024),
  readModel: async modelId => bytes(await invoke("read_vrm_skin_model", { modelId }), 64 * 1024 * 1024),
  subscribe: (name, callback) => listen(name, event => callback(event.payload)),
};

export function parseExtraPetContext(value: unknown): ExtraPetContext {
  if (!record(value) || typeof value.nativeVisible !== "boolean" || typeof value.paused !== "boolean" || !(value.png === null || record(value.png)) || !(value.vrm === null || record(value.vrm))) throw Error("辅助桌宠窗口上下文无效。");
  const pet = parseExtraPet(value.pet);
  const png = value.png === null ? null : parseLocalPetSkinPackage(value.png);
  const vrm = value.vrm === null ? null : parseVrmModel(value.vrm);
  if ((png && (pet.skinChoice.kind !== "png" || png.skinId !== pet.skinChoice.id)) ||
      (vrm && (pet.skinChoice.kind !== "vrm" || vrm.modelId !== pet.skinChoice.id))) throw Error("辅助桌宠角色资源与当前配置不匹配。");
  return { pet, nativeVisible: value.nativeVisible, paused: value.paused, png, vrm };
}
export function parseExtraPetRenderer(value: unknown, context: ExtraPetContext, epoch: string): ExtraPetRenderer {
  if (!record(value) || value.petId !== context.pet.petId || value.instanceEpoch !== context.pet.instanceEpoch || value.rendererEpoch !== epoch ||
      !uuid.test(epoch) || typeof value.token !== "string" || !uuid.test(value.token) || typeof value.generation !== "string" ||
      !/^(0|[1-9]\d{0,19})$/.test(value.generation) || BigInt(value.generation) > 18_446_744_073_709_551_615n) throw Error("辅助桌宠渲染许可无效。");
  return { petId: context.pet.petId, instanceEpoch: context.pet.instanceEpoch, rendererEpoch: epoch, token: value.token, generation: value.generation };
}
export function checkExtraPetPulse(pulse: PetPulse): PetPulse {
  if (!pulse || !Array.isArray(pulse.prompts) || pulse.prompts.length !== 0) throw Error("辅助桌宠不接受通知或完成提示。");
  return pulse;
}
export function extraPetPresentation(pulse: PetPulse | null, visible: boolean): PetPresentation | null {
  // petVisible 属于主桌宠偏好；辅助视图只服从自己的真实窗口可见性。
  return pulse ? { ...pulse, prompts: [], settings: { ...pulse.settings, petVisible: visible } } : null;
}
const emptyRead: PetReadState = { pulse: null, ready: false, details: null, detailsReady: false, connectionError: null, detailsError: null };
const configKey = (context: ExtraPetContext) => JSON.stringify([context.pet.petId, context.pet.instanceEpoch, context.pet.skinChoice, context.pet.visible, context.pet.scale]);
const retryDelays = [100, 250, 500, 1000, 1500];

export function useExtraPet(client: ExtraPetWindowClient = extraPetWindowClient) {
  const [context, setContext] = useState<ExtraPetContext | null>(null);
  const [renderer, setRenderer] = useState<ExtraPetRenderer | null>(null);
  const [read, setRead] = useState(emptyRead);
  const [error, setError] = useState("");
  const [host, setHost] = useState({ visible: false, paused: true });
  const [systemReduced, setSystemReduced] = useState(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  const controls = useRef({ refresh: () => {}, drag: (_request: ExtraPetDrag) => {}, close: async () => {} });
  useEffect(() => {
    let live = true, current: ExtraPetContext | null = null, lease: ExtraPetRenderer | null = null;
    let nativeVisible = false, paused = true, runtimeRevision = 0, windowRevision = 0, registration = 0;
    let running = false, queued = false, contextRevision = 0, retry = 0, retryTimer: number | null = null, registering = false;
    let closed = false;
    const visibility = new Map<string, { visible: boolean; revision: number }>();
    const removers: (() => void)[] = [];
    const owner = new PetReadLifecycle({ now: () => Date.now(), pulse: async cursor => checkExtraPetPulse(await client.pulse(cursor)), details: async () => { throw Error("辅助桌宠没有账本详情权限。"); } });
    setContext(null); setRenderer(null); setRead(owner.state()); setError(""); setHost({ visible: false, paused: true });
    const unsubscribe = owner.subscribe(state => { if (live) setRead(state); });
    const allowed = () => live && !closed && !!current?.pet.visible && nativeVisible && !paused && !document.hidden;
    const emitEnvironment = () => {
      if (!live) return;
      setHost({ visible: allowed(), paused });
      window.dispatchEvent(new CustomEvent("initial-view-state", { detail: { visible: allowed(), paused } }));
    };
    const release = () => {
      ++registration; registering = false;
      const previous = lease; lease = null;
      owner.setVisible(false);
      if (live) setRenderer(null);
      if (previous) void client.unregister(previous.rendererEpoch, previous.token).catch(() => {});
    };
    const register = () => {
      if (!allowed() || !current || lease || registering) return;
      registering = true;
      const ticket = ++registration, expected = current, key = configKey(current), epoch = crypto.randomUUID();
      void client.register(epoch).then(raw => {
        const incoming = parseExtraPetRenderer(raw, expected, epoch);
        if (!live || ticket !== registration || !current || configKey(current) !== key || !allowed()) {
          void client.unregister(incoming.rendererEpoch, incoming.token).catch(() => {}); return;
        }
        lease = incoming; setRenderer(incoming); setError(""); owner.setVisible(true);
      }).catch(reason => { if (live && ticket === registration) { setError(text(reason)); owner.setVisible(false); } })
        .finally(() => { if (ticket === registration) registering = false; });
    };
    const synchronize = () => {
      emitEnvironment();
      if (!allowed()) release();
      else { register(); owner.setVisible(!!lease); }
    };
    const refresh = () => {
      if (!live || closed) return;
      ++contextRevision;
      if (retryTimer !== null) { clearTimeout(retryTimer); retryTimer = null; }
      if (running) { queued = true; return; }
      running = true; queued = false;
      const requestedRuntime = runtimeRevision, requestedWindow = windowRevision, requestedContext = contextRevision;
      void client.context().then(raw => {
        // 配置事件排队时，旧快照不能短暂重领旧角色的渲染许可。
        if (!live || closed || requestedContext !== contextRevision) return;
        const incoming = parseExtraPetContext(raw);
        if (current && (current.pet.petId !== incoming.pet.petId || current.pet.instanceEpoch !== incoming.pet.instanceEpoch)) throw Error("辅助桌宠实例已变化，请在主面板重新打开。");
        const changed = !current || configKey(current) !== configKey(incoming);
        const nextPaused = runtimeRevision === requestedRuntime ? incoming.paused : paused;
        const recorded = visibility.get(incoming.pet.label);
        const nextVisible = recorded && recorded.revision > requestedWindow ? recorded.visible : incoming.nativeVisible;
        if (changed || nativeVisible !== nextVisible || paused !== nextPaused) release();
        if (changed) owner.domainChanged();
        retry = 0; nativeVisible = nextVisible; paused = nextPaused;
        current = { ...incoming, nativeVisible, paused };
        setContext(current); setError(""); synchronize();
      }).catch(reason => {
        if (!live || closed || requestedContext !== contextRevision) return;
        release();
        setError(text(reason));
        if (!current && retry < retryDelays.length) retryTimer = window.setTimeout(() => { retryTimer = null; refresh(); }, retryDelays[retry++]);
      }).finally(() => {
        running = false;
        if (queued && live && !closed) refresh();
      });
    };
    const nativeWindow = (payload: unknown) => {
      if (!record(payload) || typeof payload.label !== "string" || typeof payload.visible !== "boolean") return;
      visibility.set(payload.label, { visible: payload.visible, revision: ++windowRevision });
      if (visibility.size > 8) visibility.delete(visibility.keys().next().value!);
      if (current?.pet.label !== payload.label) return;
      nativeVisible = payload.visible; synchronize();
    };
    const runtime = (payload: unknown) => {
      if (!record(payload) || typeof payload.paused !== "boolean") return;
      ++runtimeRevision; paused = payload.paused;
      release();
      window.dispatchEvent(new CustomEvent("desktop-runtime-state", { detail: { paused } }));
      synchronize();
      if (!paused) refresh();
    };
    const documentVisibility = () => { synchronize(); if (!document.hidden) refresh(); };
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const preference = () => { if (live) setSystemReduced(media.matches); };
    document.addEventListener("visibilitychange", documentVisibility); media.addEventListener("change", preference);
    const events: [string, (payload: unknown) => void][] = [
      ["extra-pets-changed", payload => { if (!live) return; if (!record(payload) || payload.kind !== "position") release(); refresh(); }],
      ["window-visibility-changed", nativeWindow], ["desktop-runtime-state", runtime],
      ["ledger-updated", payload => { if (record(payload) && payload.petDomainChanged === true) owner.domainChanged(); else void owner.request(); }],
      ["pet-options-changed", () => owner.settingsChanged()],
    ];
    void Promise.all(events.map(async ([name, callback]) => {
      const remove = await client.subscribe(name, payload => { if (live) callback(payload); });
      if (!live) remove(); else removers.push(remove);
    })).then(() => { if (live) refresh(); }).catch(reason => { if (live) { release(); setError(text(reason)); } });
    controls.current = {
      refresh() { retry = 0; refresh(); },
      drag(request) {
        const expected = lease;
        if (!expected || !allowed()) return;
        void client.drag(request, expected).catch(reason => { if (live && lease?.token === expected.token) setError(text(reason)); });
      },
      async close() {
        const expected = lease;
        if (!expected) { if (live) setError("尚未取得当前窗口许可，请在主面板关闭这个辅助桌宠。"); return; }
        try { await client.close(expected); if (live && lease?.token === expected.token) { closed = true; release(); } }
        catch (reason) { if (live && lease?.token === expected.token) setError(text(reason)); }
      },
    };
    return () => {
      live = false; closed = true; release();
      if (retryTimer !== null) clearTimeout(retryTimer);
      document.removeEventListener("visibilitychange", documentVisibility); media.removeEventListener("change", preference);
      removers.forEach(remove => remove()); unsubscribe(); owner.dispose();
    };
  }, [client]);
  const visible = host.visible && !!renderer && !host.paused;
  return { context, renderer, read, error, host: { visible, paused: host.paused },
    reducedMotion: systemReduced || read.pulse?.settings.reducedMotion === true,
    presentation: extraPetPresentation(read.pulse, visible), ready: read.ready && visible,
    refresh: () => controls.current.refresh(), drag: (request: ExtraPetDrag) => controls.current.drag(request), close: () => controls.current.close() };
}
