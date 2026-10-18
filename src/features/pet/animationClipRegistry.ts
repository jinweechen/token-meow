import { inspectPng } from "./png";

export interface AnimationFrameDefinition {
  id: string;
  path: string;
  width: number;
  height: number;
  bytes: number;
  sha256: string;
  bounds: readonly [number, number, number, number];
}
export interface AnimationClipDefinition {
  id: string;
  fallback: string;
  sequence: readonly { frame: string; durationMs: number }[];
}
export interface AnimationFrameAsset { readonly id: string; readonly url: string }
export interface AnimationClipAsset {
  readonly clip: string;
  readonly frames: Readonly<Record<string, AnimationFrameAsset>>;
}
interface DecodedFrame {
  width: number;
  height: number;
  corners: readonly number[];
  close(): void;
}
export interface AnimationClipHost {
  fetch(source: string, signal: AbortSignal): Promise<Response>;
  decode(blob: Blob): Promise<DecodedFrame>;
  createUrl(blob: Blob): string;
  revokeUrl(url: string): void;
}
interface LoadRequest {
  clip: string;
  staticOnly: boolean;
  pending: boolean;
  started: boolean;
  generation: number;
  controller: AbortController;
  ready: Promise<AnimationClipAsset | null>;
  resolve(asset: AnimationClipAsset | null): void;
  reject(error: unknown): void;
}

export const ANIMATION_ASSET_LIMITS = Object.freeze({ frames: 64, clips: 16,
  clipFrames: 12, encodedBytes: 128 * 1024 * 1024 });
const MAX_CLIP_FRAMES = ANIMATION_ASSET_LIMITS.clipFrames;
const RGBA_BYTES = 1024 * 1536 * 4;
const validId = (id: string) => typeof id === "string" &&
  /^[a-z0-9][a-z0-9/_-]{0,127}$/.test(id) && !id.includes("//");

export const browserAnimationClipHost: AnimationClipHost = {
  fetch(source, signal) {
    const origin = new URL(window.location.href);
    const url = new URL(source, origin);
    if (!["http:", "https:", "tauri:"].includes(url.protocol) ||
      url.protocol !== origin.protocol || url.host !== origin.host ||
      url.username || url.password) throw Error("动画帧只允许同源内置资源");
    return fetch(url, { signal, redirect: "error", credentials: "same-origin" });
  },
  async decode(blob) {
    const bitmap = await createImageBitmap(blob);
    try {
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 1;
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (!context) throw Error("无法检查动画帧透明通道");
      const corners = [[0, 0], [bitmap.width - 1, 0],
        [0, bitmap.height - 1], [bitmap.width - 1, bitmap.height - 1]].map(([x, y]) => {
        context.clearRect(0, 0, 1, 1);
        context.drawImage(bitmap, x, y, 1, 1, 0, 0, 1, 1);
        return context.getImageData(0, 0, 1, 1).data[3];
      });
      return { width: bitmap.width, height: bitmap.height, corners,
        close: () => bitmap.close() };
    } catch (error) {
      bitmap.close();
      throw error;
    }
  },
  createUrl: (blob) => URL.createObjectURL(blob),
  revokeUrl: (url) => URL.revokeObjectURL(url),
};

async function readBytes(response: Response, size: number, signal: AbortSignal) {
  if (!response.ok || response.redirected || !response.body) {
    await response.body?.cancel().catch(() => {});
    throw Error("无法读取动画帧资源");
  }
  const reader = response.body.getReader();
  const cancel = () => { void reader.cancel().catch(() => {}); };
  signal.addEventListener("abort", cancel, { once: true });
  const bytes = new Uint8Array(size);
  let offset = 0;
  try {
    if (signal.aborted) throw new DOMException("已取消动画帧读取", "AbortError");
    for (;;) {
      const { done, value } = await reader.read();
      if (signal.aborted) throw new DOMException("已取消动画帧读取", "AbortError");
      if (done) break;
      if (value.byteLength > size - offset) {
        await reader.cancel().catch(() => {});
        throw Error("动画帧资源长度超出清单");
      }
      bytes.set(value, offset);
      offset += value.byteLength;
    }
    if (offset !== size) throw Error("动画帧资源长度与清单不符");
    return bytes;
  } finally {
    signal.removeEventListener("abort", cancel);
    reader.releaseLock();
  }
}

/** One window owns one active clip. Only its distinct frames are read, serially.
 * A transition releases every old URL before allocating the next decode slot.
 * The twelve URL/decode slots are a logical 72 MiB budget, not a browser RAM claim.
 */
export class AnimationClipRegistry {
  private frames = new Map<string, AnimationFrameDefinition>();
  private clips = new Map<string, AnimationClipDefinition>();
  private sources = new Map<string, string>();
  private cache = new Map<string, AnimationFrameAsset>();
  private disposed = false;
  private generation = 0;
  private decoding = 0;
  private draining = false;
  private current: LoadRequest | null = null;

  constructor(frames: readonly AnimationFrameDefinition[], sources: Record<string, string>,
    clips: readonly AnimationClipDefinition[], private host: AnimationClipHost = browserAnimationClipHost) {
    if (!frames.length || frames.length > ANIMATION_ASSET_LIMITS.frames ||
        !clips.length || clips.length > ANIMATION_ASSET_LIMITS.clips)
      throw Error("动画清单数量无效");
    let bytes = 0;
    const paths = new Set<string>();
    for (const frame of frames) {
      const [left, top, right, bottom] = frame.bounds ?? [];
      if (!validId(frame.id) || this.frames.has(frame.id) ||
        !/^assets\/pets\/[a-z0-9/_-]+\.png$/.test(frame.path) || paths.has(frame.path) ||
        frame.width !== 1024 || frame.height !== 1536 ||
        !Number.isSafeInteger(frame.bytes) || frame.bytes < 45 || frame.bytes > 4 * 1024 * 1024 ||
        !/^[a-f0-9]{64}$/.test(frame.sha256) ||
        !Object.hasOwn(sources, frame.id) || typeof sources[frame.id] !== "string" || !sources[frame.id] ||
        frame.bounds?.length !== 4 || frame.bounds.some((n) => !Number.isSafeInteger(n)) ||
        left < 0 || top < 0 || right <= left || bottom <= top ||
        right > frame.width || bottom > frame.height) throw Error("动画帧清单、范围或资源无效");
      bytes += frame.bytes;
      paths.add(frame.path);
      this.frames.set(frame.id, { ...frame, bounds: [...frame.bounds] });
      this.sources.set(frame.id, sources[frame.id]);
    }
    if (bytes > ANIMATION_ASSET_LIMITS.encodedBytes) throw Error("动画素材包超过128 MiB预算");
    for (const clip of clips) {
      if (!validId(clip.id) || this.clips.has(clip.id) || !this.frames.has(clip.fallback) ||
        !Array.isArray(clip.sequence) || !clip.sequence.length || clip.sequence.length > 64 ||
        clip.sequence.some((step) => !this.frames.has(step.frame) ||
          !Number.isSafeInteger(step.durationMs) || step.durationMs < 1 || step.durationMs > 60_000) ||
        new Set([clip.fallback, ...clip.sequence.map((step) => step.frame)]).size > MAX_CLIP_FRAMES)
        throw Error("动画片段清单或十二帧预算无效");
      this.clips.set(clip.id, { id: clip.id, fallback: clip.fallback,
        sequence: clip.sequence.map((step) => ({ ...step })) });
    }
  }

  load(clip: string, staticOnly = false): Promise<AnimationClipAsset | null> {
    if (this.disposed) return Promise.resolve(null);
    if (!this.clips.has(clip)) {
      this.cancel();
      return Promise.reject(Error("未知动画片段"));
    }
    if (this.current?.clip === clip && this.current.staticOnly === staticOnly) return this.current.ready;
    const generation = ++this.generation;
    this.cancelCurrent();
    this.releaseUrls();
    const controller = new AbortController();
    let resolve!: LoadRequest["resolve"], reject!: LoadRequest["reject"];
    const ready = new Promise<AnimationClipAsset | null>((done, failed) => {
      resolve = done;
      reject = failed;
    });
    const request: LoadRequest = { clip, staticOnly, pending: true, started: false,
      generation, controller, ready, resolve, reject };
    this.current = request;
    void this.drain();
    return ready;
  }

  private cancelCurrent() {
    const request = this.current;
    this.current = null;
    request?.controller.abort();
    if (request?.pending) {
      request.pending = false;
      request.resolve(null);
    }
  }

  /** Keep at most the executing read and its latest replacement. Superseded
   * callers settle immediately; a blocked host cannot grow a promise work queue.
   */
  private async drain() {
    if (this.draining) return;
    this.draining = true;
    try {
      for (;;) {
        const request = this.current;
        if (!request || !request.pending || request.started) break;
        request.started = true;
        try {
          const asset = await this.readClip(request.clip, request.staticOnly,
            request.generation, request.controller.signal);
          if (this.current === request) {
            request.pending = false;
            if (!asset) this.current = null;
            request.resolve(asset);
          }
        } catch (error) {
          if (this.current === request) {
            request.pending = false;
            this.current = null;
            request.reject(error);
          }
        }
      }
    } finally {
      this.draining = false;
    }
  }

  private valid(generation: number) {
    return !this.disposed && generation === this.generation;
  }

  private releaseUrls() {
    for (const asset of this.cache.values()) this.host.revokeUrl(asset.url);
    this.cache.clear();
  }

  private async readClip(clipId: string, staticOnly: boolean, generation: number, signal: AbortSignal) {
    if (!this.valid(generation)) return null;
    const clip = this.clips.get(clipId)!;
    const ids = staticOnly ? [clip.fallback] : [...new Set([clip.fallback,
      ...clip.sequence.map((step) => step.frame)])];
    try {
      for (const id of ids) {
        if (!this.valid(generation)) return null;
        const frame = this.frames.get(id)!;
        const response = await this.host.fetch(this.sources.get(id)!, signal);
        if (!this.valid(generation)) {
          await response.body?.cancel().catch(() => {});
          return null;
        }
        const bytes = await readBytes(response, frame.bytes, signal);
        const dimensions = inspectPng(bytes);
        if (dimensions.width !== frame.width || dimensions.height !== frame.height)
          throw Error("动画帧PNG尺寸与清单不符");
        const digest = await crypto.subtle.digest("SHA-256", bytes);
        const sha256 = [...new Uint8Array(digest)]
          .map((byte) => byte.toString(16).padStart(2, "0")).join("");
        if (sha256 !== frame.sha256) throw Error("动画帧完整性校验失败");
        if (!this.valid(generation)) return null;
        if (this.cache.size >= MAX_CLIP_FRAMES) throw Error("动画解码槽预算超限");
        const blob = new Blob([bytes.buffer], { type: "image/png" });
        this.decoding = 1;
        try {
          const bitmap = await this.host.decode(blob);
          try {
            if (!this.valid(generation)) return null;
            // Approved originals include isolated 1/255 edge residue. Their exact
            // bytes are SHA-bound above; no alpha or source pixels are rewritten.
            if (bitmap.width !== frame.width || bitmap.height !== frame.height ||
              bitmap.corners.length !== 4 || !bitmap.corners.every((alpha) =>
                Number.isInteger(alpha) && alpha >= 0 && alpha <= 1))
              throw Error("动画帧解码尺寸或透明边界无效");
          } finally {
            bitmap.close();
          }
        } finally {
          this.decoding = 0;
        }
        if (!this.valid(generation)) return null;
        const url = this.host.createUrl(blob);
        if (!this.valid(generation)) {
          this.host.revokeUrl(url);
          return null;
        }
        this.cache.set(id, Object.freeze({ id, url }));
      }
      return Object.freeze({ clip: clipId,
        frames: Object.freeze(Object.fromEntries(this.cache)) });
    } catch (error) {
      if (!this.valid(generation)) return null;
      this.releaseUrls();
      throw error;
    }
  }

  diagnostics() {
    return {
      disposed: this.disposed, clip: this.current?.clip ?? null,
      staticOnly: this.current?.staticOnly ?? false, pending: this.current?.pending ?? false,
      queuedRequests: Number(!!this.current?.pending && !this.current.started),
      pendingDecodes: this.decoding, objectUrls: this.cache.size,
      imageSlots: this.cache.size + this.decoding,
      cachedBytes: [...this.cache.keys()].reduce((sum, id) => sum + this.frames.get(id)!.bytes, 0),
      decodedBytes: (this.cache.size + this.decoding) * RGBA_BYTES,
    };
  }

  /** Suspend the current view without ending its reusable window owner. */
  cancel() {
    if (this.disposed) return;
    this.generation++;
    this.cancelCurrent();
    this.releaseUrls();
  }

  dispose() {
    if (this.disposed) return;
    this.cancel();
    this.disposed = true;
  }
}
