import { LOCAL_PET_POSES, parseLocalPetSkinPackage, type LocalPetSkinClient,
  type LocalPetSkinClip, type LocalPetSkinPackage } from "../../shared/localPetSkin";
import { AnimationClipRegistry, browserAnimationClipHost, type AnimationClipHost } from "./animationClipRegistry";
import { inspectPng } from "./png";

export interface LocalPetSkinRuntime {
  readonly key: string;
  readonly skinId: string;
  readonly manifest: LocalPetSkinPackage["manifest"];
  readonly clips: readonly LocalPetSkinClip[];
  readonly gaitAvailable: false;
  getRegistry(): AnimationClipRegistry;
  reportFailure(): void;
  setBeforeRelease(gate: (() => Promise<void>) | undefined): void;
  cancel(): void;
  dispose(): void;
}

function checkAbort(signal: AbortSignal) {
  if (signal.aborted) throw new DOMException("已取消皮肤检查", "AbortError");
}

/** Opaque reads only: manifest paths are never opened by the browser. */
export function createLocalPetSkinRuntime(input: LocalPetSkinPackage, client: LocalPetSkinClient,
  onFailure: () => void = () => {}, imageHost = browserAnimationClipHost): LocalPetSkinRuntime {
  const pkg = parseLocalPetSkinPackage(input);
  const idle = pkg.manifest.clips.find((clip) => clip.id === "idle")!;
  const clips = Object.freeze(LOCAL_PET_POSES.map((id) =>
    pkg.manifest.clips.find((clip) => clip.id === id) ?? Object.freeze({ ...idle, id })));
  const sources: Record<string, string> = {};
  const allowed = new Map<string, string>();
  const frames = pkg.manifest.frames.map((frame) => {
    const source = `local-png:${pkg.skinId}/${frame.id}`;
    sources[frame.id] = source;
    allowed.set(source, frame.id);
    return { ...frame, path: `assets/pets/local/${pkg.skinId}/${frame.id}.png` };
  });
  const host: AnimationClipHost = {
    ...imageHost,
    async fetch(source, signal) {
      checkAbort(signal);
      const frameId = allowed.get(source);
      if (!frameId) throw Error("未知本地皮肤帧");
      const bytes = await client.readFrame(pkg.skinId, frameId);
      checkAbort(signal);
      return new Response(bytes, { headers: { "Content-Type": "image/png" } });
    },
  };
  let registry: AnimationClipRegistry | null = null;
  let disposed = false;
  let beforeRelease: (() => Promise<void>) | undefined;
  return {
    key: `local:${pkg.skinId}`, skinId: pkg.skinId, manifest: pkg.manifest, clips, gaitAvailable: false,
    getRegistry() {
      if (disposed) throw Error("本地皮肤窗口已释放");
      return registry ??= new AnimationClipRegistry(frames, sources, clips, host);
    },
    reportFailure() { if (!disposed) onFailure(); },
    setBeforeRelease(gate) { beforeRelease = gate; },
    cancel() { registry?.cancel(); },
    dispose() {
      if (disposed) return;
      disposed = true;
      const owned = registry; registry = null;
      if (beforeRelease) void Promise.resolve().then(beforeRelease).catch(() => {}).finally(() => owned?.dispose());
      else owned?.dispose();
    },
  };
}

/** Full browser compatibility check before Use, serially, without retaining URLs/bitmaps.
 * Backend separately verifies CRC, decoded alpha>127 bounds, and package storage.
 */
export async function validateLocalPetSkinFrames(input: LocalPetSkinPackage, client: LocalPetSkinClient,
  signal: AbortSignal, progress: (checked: number, total: number) => void = () => {},
  imageHost = browserAnimationClipHost): Promise<void> {
  const pkg = parseLocalPetSkinPackage(input);
  progress(0, pkg.manifest.frames.length);
  let checked = 0;
  for (const frame of pkg.manifest.frames) {
    checkAbort(signal);
    const buffer = await client.readFrame(pkg.skinId, frame.id);
    checkAbort(signal);
    if (!(buffer instanceof ArrayBuffer) || buffer.byteLength !== frame.bytes) throw Error("皮肤帧长度与清单不符");
    const bytes = new Uint8Array(buffer), dimensions = inspectPng(bytes);
    if (dimensions.width !== frame.width || dimensions.height !== frame.height) throw Error("皮肤帧尺寸与清单不符");
    const hash = [...new Uint8Array(await crypto.subtle.digest("SHA-256", buffer))]
      .map((value) => value.toString(16).padStart(2, "0")).join("");
    checkAbort(signal);
    if (hash !== frame.sha256) throw Error("皮肤帧完整性检查失败");
    const bitmap = await imageHost.decode(new Blob([buffer], { type: "image/png" }));
    try {
      checkAbort(signal);
      if (bitmap.width !== frame.width || bitmap.height !== frame.height || bitmap.corners.length !== 4 ||
        !bitmap.corners.every((alpha) => Number.isInteger(alpha) && alpha >= 0 && alpha <= 1))
        throw Error("皮肤帧无法解码或透明边界无效");
    } finally { bitmap.close(); }
    progress(++checked, pkg.manifest.frames.length);
  }
}
