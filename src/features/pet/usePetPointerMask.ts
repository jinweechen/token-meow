import { useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "../../shared/bridge";
import { ALPHA_MASK_RENEW_MS, buildConservativeAlphaMask } from "./alphaMask";

type Viewport = { width: number; height: number; dpi: number };
type Owner = { owner: number; viewport: Viewport | null };
type Frame = Owner & { viewport: Viewport; revision: number; frameKey: string };
type Session = {
  live: boolean; context: string; ready: Promise<Owner>; frame: Frame | null;
  published: Frame | null; needsBegin: boolean; revision: number;
  queue: Promise<unknown>; begin: (frameKey: string) => Promise<void>;
};
const UI_SELECTOR = ".pet-status-badge,.pet-health-badge,.pet-companion-context,.pet-affection-button,.pet-care-bubble,.pet-popover,.pet-preview-tag,.pet-toast";

// Every image carries its COMMITTED state's data-pointer-frame-key. A pending
// animation ACK never authorizes pixels from an older DOM image or source scope.
export function usePetPointerMask(enabled: boolean, context: string) {
  const characterRef = useRef<HTMLDivElement>(null);
  const session = useRef<Session | null>(null);
  const beforeDisplay = useCallback(async (frameKey: string) => {
    if (!enabled || !isDesktop) return;
    const current = session.current;
    if (!current?.live || current.context !== context)
      throw Error("透明命中渲染域已失效");
    await current.begin(frameKey);
  }, [enabled, context]);

  useEffect(() => {
    if (!enabled || !isDesktop) return;
    const current: Session = {
      live: true, context, ready: invoke<Owner>("register_pet_pointer_renderer"),
      frame: null, published: null, needsBegin: false, revision: 0,
      queue: Promise.resolve(), begin: async () => {},
    };
    session.current = current;
    let scheduled = 0, drawing = false, again = false, layoutQueued = false, layoutEpoch = 0;
    let resize: ResizeObserver;
    const observedSizes = new Set<Element>();
    const root = () => characterRef.current;
    const image = () => root()?.querySelector<HTMLImageElement>(".pet-hd-sprite img") ?? null;
    const domKey = () => image()?.dataset.pointerFrameKey ?? null;
    const enqueue = <T,>(action: () => Promise<T>): Promise<T> => {
      const operation = current.queue.catch(() => {}).then(action);
      current.queue = operation;
      return operation;
    };
    const schedule = () => {
      if (current.live && !scheduled)
        scheduled = window.requestAnimationFrame(() => { void publish(); });
    };
    const begin = async (frameKey: string) => {
      const owner = await current.ready;
      if (!current.live) throw Error("透明命中渲染域已失效");
      // Do not truncate identity: distinct source keys must not become one key.
      if (!frameKey || new TextEncoder().encode(frameKey).length > 256)
        throw Error("透明命中帧标识无效");
      current.frame = null;
      current.published = null;
      current.needsBegin = false;
      const revision = ++current.revision;
      const { viewport } = await invoke<{ viewport: Viewport }>("begin_pet_pointer_frame", {
        owner: owner.owner, revision, frameKey,
      });
      if (!current.live) throw Error("透明命中渲染域已失效");
      if (!viewport || !Number.isSafeInteger(viewport.width) || !Number.isSafeInteger(viewport.height) ||
          !Number.isSafeInteger(viewport.dpi) || viewport.width < 1 || viewport.height < 1 ||
          viewport.width > 8192 || viewport.height > 8192 || viewport.dpi < 48 || viewport.dpi > 768)
        throw Error("透明命中窗口尺寸暂不可用");
      current.frame = { ...owner, viewport, revision, frameKey };
      schedule();
    };
    current.begin = (frameKey) => enqueue(() => begin(frameKey));

    const observeSizes = () => {
      const wanted = new Set<Element>(document.querySelectorAll(UI_SELECTOR));
      const character = root();
      if (character) wanted.add(character);
      for (const element of observedSizes) {
        if (!wanted.has(element)) { resize.unobserve(element); observedSizes.delete(element); }
      }
      for (const element of wanted) {
        if (!observedSizes.has(element)) { resize.observe(element); observedSizes.add(element); }
      }
    };
    // Read DOM identity at EXECUTION, after older ACKs. An A-layout event cannot
    // enqueue a new A after frame B has already obtained its native ACK.
    const layout = () => {
      layoutEpoch += 1;
      if (!current.live || layoutQueued) return;
      layoutQueued = true;
      void enqueue(async () => {
        try {
          if (!current.live) return;
          const key = domKey();
          if (!key || !current.frame || current.frame.frameKey !== key) return;
          await begin(key);
          if (current.live) observeSizes();
        } finally { layoutQueued = false; }
      }).catch(() => { current.needsBegin = true; });
    };
    const publish = async () => {
      scheduled = 0;
      if (drawing) { again = true; return; }
      drawing = true;
      let canvas: HTMLCanvasElement | null = null;
      let attemptedFrame: Frame | null = null;
      try {
        const frame = current.frame;
        attemptedFrame = frame;
        const epoch = layoutEpoch;
        const img = image();
        if (!current.live || !frame || current.published === frame || current.needsBegin ||
            !img?.complete || !img.naturalWidth || document.hidden || domKey() !== frame.frameKey) return;
        const { width, height } = frame.viewport;
        if (width < 1 || height < 1 || width > 4096 || height > 4096 || width * height > 8_388_608 ||
            window.innerWidth <= 0 || window.innerHeight <= 0) return;
        const sx = width / window.innerWidth, sy = height / window.innerHeight;
        canvas = document.createElement("canvas");
        canvas.width = width; canvas.height = height;
        const ctx = canvas.getContext("2d", { willReadFrequently: true });
        const stage = img.parentElement!.getBoundingClientRect();
        const bounds = img.getBoundingClientRect();
        if (!ctx || stage.width < 1 || stage.height < 1 || bounds.width < 1 || bounds.height < 1) return;
        ctx.beginPath(); ctx.rect(stage.left * sx, stage.top * sy, stage.width * sx, stage.height * sy); ctx.clip();
        ctx.drawImage(img, bounds.left * sx, bounds.top * sy, bounds.width * sx, bounds.height * sy);
        const grid = buildConservativeAlphaMask(ctx.getImageData(0, 0, width, height).data, width, height);
        const uiRects = Array.from(document.querySelectorAll(UI_SELECTOR))
          .map(element => element.getBoundingClientRect()).filter(r => r.width > 0 && r.height > 0)
          .map(r => ({ left: Math.max(0, Math.floor(r.left * sx)), top: Math.max(0, Math.floor(r.top * sy)),
            right: Math.min(width, Math.ceil(r.right * sx)), bottom: Math.min(height, Math.ceil(r.bottom * sy)) }))
          .filter(r => r.right > r.left && r.bottom > r.top);
        if (uiRects.length > 16) return; // Never truncate interactive UI.
        await enqueue(async () => {
          if (!current.live || current.frame !== frame || domKey() !== frame.frameKey || layoutEpoch !== epoch) return;
          const published = await invoke<boolean>("publish_pet_pointer_mask", { mask: {
            owner: frame.owner, revision: frame.revision, frameKey: frame.frameKey,
            viewport: frame.viewport, ...grid, uiRects,
          } });
          if (!published) throw Error("透明命中掩码未接受");
          if (current.live && current.frame === frame) current.published = frame;
        });
      } catch { if (current.frame === attemptedFrame) current.needsBegin = true; }
      finally {
        if (canvas) canvas.width = canvas.height = 0;
        drawing = false;
        if (again) { again = false; schedule(); }
      }
    };

    const observer = new MutationObserver(layout);
    const page = root()?.closest(".pet-window");
    if (page) observer.observe(page, { childList: true, characterData: true, subtree: true, attributes: true,
      attributeFilter: ["src", "style", "class", "data-pointer-frame-key"] });
    resize = new ResizeObserver(layout);
    observeSizes();
    const load = (event: Event) => { if (event.target === image()) layout(); };
    document.addEventListener("load", load, true);
    window.addEventListener("resize", layout);
    window.addEventListener("pet-interaction", layout);
    document.fonts?.addEventListener("loadingdone", layout);
    const timer = window.setInterval(() => {
      const frame = current.frame;
      if (!current.live || !frame || document.hidden || domKey() !== frame.frameKey) return;
      if (current.needsBegin) { layout(); return; }
      if (current.published !== frame) { schedule(); return; }
      void enqueue(async () => {
        if (!current.live || current.frame !== frame || domKey() !== frame.frameKey) return;
        try {
          const renewed = await invoke<boolean>("renew_pet_pointer_mask", { owner: frame.owner, revision: frame.revision });
          if (!renewed) throw Error("透明命中租约已到期");
        } catch {
          if (current.live && current.frame === frame) { current.needsBegin = true; layout(); }
        }
      });
    }, ALPHA_MASK_RENEW_MS);
    current.ready.catch(() => {});
    return () => {
      current.live = false;
      if (session.current === current) session.current = null;
      observer.disconnect(); resize.disconnect(); window.clearInterval(timer); window.cancelAnimationFrame(scheduled);
      document.removeEventListener("load", load, true); window.removeEventListener("resize", layout);
      window.removeEventListener("pet-interaction", layout); document.fonts?.removeEventListener("loadingdone", layout);
      void current.ready.then(({ owner }) => invoke("release_pet_pointer_renderer", { owner })).catch(() => {});
    };
  }, [enabled, context]);
  return { beforeDisplay, characterRef };
}
