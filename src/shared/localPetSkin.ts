/** Local PNG packages are data only. Paths never become browser or IPC file paths. */
export const LOCAL_PET_SKIN_LIMITS = Object.freeze({
  width: 1024, height: 1536, frames: 64, clips: 16, clipFrames: 12,
  frameBytes: 4 * 1024 * 1024, encodedBytes: 128 * 1024 * 1024, installed: 8,
});

export const LOCAL_PET_POSES = Object.freeze([
  "idle", "active", "complete", "alert", "sleep", "shy", "affection", "stretch",
  "wink", "happy", "bow", "hug-tail", "walk",
] as const);
export type LocalPetPose = typeof LOCAL_PET_POSES[number];
export interface LocalPetSkinFrame {
  id: string; path: string; width: 1024; height: 1536; bytes: number; sha256: string;
  bounds: readonly [number, number, number, number];
}
export interface LocalPetSkinClip {
  id: LocalPetPose;
  fallback: string;
  canvas: readonly [1024, 1536];
  sharedTransform: { scale: 1; anchor: readonly [512, 1490]; translate: readonly [0, 0] };
  loop: boolean;
  loopRange?: { start: number; end: number };
  sequence: readonly { frame: string; durationMs: number; segment?: string }[];
}
export interface LocalPetSkinManifestV1 {
  schema_version: 1;
  renderer_kind: "local-png-clips";
  bounds_alpha_threshold: 127;
  name: string; author: string; source: string; license: string; attribution: string;
  static_pose: "idle";
  frames: readonly LocalPetSkinFrame[];
  clips: readonly LocalPetSkinClip[];
}
export interface LocalPetSkinPackage {
  /** Backend-assigned SHA-256 of the original manifest bytes; never a source path. */
  skinId: string;
  manifest: LocalPetSkinManifestV1;
}
export interface LocalPetSkinState {
  selectedSkinId: string | null;
  skins: readonly LocalPetSkinPackage[];
  problems?: readonly { skinId: string | null; message: string }[];
}
export interface LocalPetSkinClient {
  getState(): Promise<LocalPetSkinState>;
  chooseImport(): Promise<LocalPetSkinPackage | null>;
  select(skinId: string | null): Promise<LocalPetSkinState>;
  remove(skinId: string): Promise<LocalPetSkinState>;
  readFrame(skinId: string, frameId: string): Promise<ArrayBuffer>;
  subscribe?(changed: (event?: { kind: "selected" | "imported" | "removed" }) => void): Promise<() => void>;
}
export type LocalPetSkinInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

const idPattern = /^[a-z0-9][a-z0-9_-]{0,63}$/;
const shaPattern = /^[a-f0-9]{64}$/;
const forbiddenText = /[\p{Cc}\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/u;
function require(value: unknown, message: string): asserts value {
  if (!value) throw Error(message);
}
function object(value: unknown): Record<string, unknown> {
  require(!!value && typeof value === "object" && !Array.isArray(value), "本地皮肤清单必须是对象");
  return value as Record<string, unknown>;
}
function keys(value: Record<string, unknown>, required: readonly string[], optional: readonly string[] = []) {
  require(required.every((key) => Object.hasOwn(value, key)) &&
    Object.keys(value).every((key) => required.includes(key) || optional.includes(key)), "本地皮肤清单含缺失或不支持的字段");
}
function text(value: unknown, limit: number, label: string): asserts value is string {
  require(typeof value === "string" && value.trim().length > 0 && value === value.trim() &&
    new TextEncoder().encode(value).byteLength <= limit && !forbiddenText.test(value), `${label}为空、过长或含不可显示字符`);
}
function tuple(value: unknown, wanted: readonly number[]) {
  return Array.isArray(value) && value.length === wanted.length && value.every((n, index) => n === wanted[index]);
}
function freeze<T>(value: T): T {
  if (value && typeof value === "object") {
    for (const child of Object.values(value)) freeze(child);
    Object.freeze(value);
  }
  return value;
}

/** Recheck the strict data contract before it can drive a renderer. */
export function parseLocalPetSkinManifest(input: unknown): LocalPetSkinManifestV1 {
  const manifest = object(input);
  keys(manifest, ["schema_version", "renderer_kind", "bounds_alpha_threshold", "name", "author", "source", "license", "attribution", "static_pose", "frames", "clips"]);
  require(manifest.schema_version === 1 && manifest.renderer_kind === "local-png-clips" &&
    manifest.static_pose === "idle" && manifest.bounds_alpha_threshold === 127, "本地皮肤版本、渲染类型或静态姿态不支持");
  text(manifest.name, 128, "皮肤名称"); text(manifest.author, 256, "作者");
  text(manifest.source, 1024, "素材来源"); text(manifest.license, 256, "许可声明");
  text(manifest.attribution, 2048, "署名说明");
  require(Array.isArray(manifest.frames) && manifest.frames.length > 0 &&
    manifest.frames.length <= LOCAL_PET_SKIN_LIMITS.frames, "本地皮肤帧数无效");
  const frameIds = new Set<string>();
  let bytes = 0;
  for (const inputFrame of manifest.frames) {
    const frame = object(inputFrame);
    keys(frame, ["id", "path", "width", "height", "bytes", "sha256", "bounds"]);
    require(typeof frame.id === "string" && idPattern.test(frame.id) && !/^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/.test(frame.id) && !frameIds.has(frame.id), "本地皮肤帧标识无效或重复");
    require(frame.path === `frames/${frame.id}.png` && frame.width === 1024 && frame.height === 1536,
      "本地皮肤帧路径或1024×1536画布无效");
    require(Number.isSafeInteger(frame.bytes) && Number(frame.bytes) >= 45 &&
      Number(frame.bytes) <= LOCAL_PET_SKIN_LIMITS.frameBytes && typeof frame.sha256 === "string" && shaPattern.test(frame.sha256), "本地皮肤帧长度或摘要无效");
    require(Array.isArray(frame.bounds) && frame.bounds.length === 4 && frame.bounds.every(Number.isSafeInteger), "本地皮肤轮廓范围无效");
    const [left, top, right, bottom] = frame.bounds as number[];
    require(left >= 0 && top >= 0 && right > left && bottom > top && right <= 1024 && bottom <= 1536, "本地皮肤轮廓超出画布");
    frameIds.add(frame.id);
    bytes += Number(frame.bytes);
  }
  require(bytes <= LOCAL_PET_SKIN_LIMITS.encodedBytes, "本地皮肤包超过128 MiB预算");
  require(Array.isArray(manifest.clips) && manifest.clips.length > 0 &&
    manifest.clips.length <= LOCAL_PET_SKIN_LIMITS.clips, "本地皮肤片段数无效");
  const clips = new Set<string>();
  const referenced = new Set<string>();
  for (const inputClip of manifest.clips) {
    const clip = object(inputClip);
    keys(clip, ["id", "fallback", "canvas", "sharedTransform", "loop", "sequence"], ["loopRange"]);
    require(typeof clip.id === "string" && LOCAL_PET_POSES.includes(clip.id as LocalPetPose) && !clips.has(clip.id), "本地皮肤片段标识无效或重复");
    require(typeof clip.fallback === "string" && frameIds.has(clip.fallback) && tuple(clip.canvas, [1024, 1536]), "本地皮肤片段静态帧或画布无效");
    const transform = object(clip.sharedTransform);
    keys(transform, ["scale", "anchor", "translate"]);
    require(transform.scale === 1 && tuple(transform.anchor, [512, 1490]) && tuple(transform.translate, [0, 0]), "本地皮肤v1仅支持原画布变换");
    require(typeof clip.loop === "boolean" && Array.isArray(clip.sequence) && clip.sequence.length > 0 && clip.sequence.length <= 64, "本地皮肤片段序列无效");
    require((clip.id !== "idle" || clip.loop) && (clip.id !== "complete" || !clip.loop), "本地皮肤待机与完成动作的循环方式无效");
    const used = new Set<string>([clip.fallback]);
    let duration = 0;
    for (const inputStep of clip.sequence) {
      const step = object(inputStep);
      keys(step, ["frame", "durationMs"], ["segment"]);
      require(typeof step.frame === "string" && frameIds.has(step.frame) && Number.isSafeInteger(step.durationMs) &&
        Number(step.durationMs) >= 16 && Number(step.durationMs) <= 10_000, "本地皮肤片段帧或时长无效");
      if (step.segment !== undefined) text(step.segment, 64, "片段阶段");
      used.add(step.frame);
      duration += Number(step.durationMs);
    }
    require(duration <= 60_000, "本地皮肤单个片段超过60秒");
    for (const id of used) referenced.add(id);
    require(used.size <= LOCAL_PET_SKIN_LIMITS.clipFrames, "本地皮肤片段超过12个解码槽");
    if (clip.loopRange !== undefined) {
      const range = object(clip.loopRange);
      keys(range, ["start", "end"]);
      require(clip.loop && Number.isSafeInteger(range.start) && Number.isSafeInteger(range.end) &&
        Number(range.start) >= 0 && Number(range.end) > Number(range.start) && Number(range.end) <= clip.sequence.length, "本地皮肤循环范围无效");
    }
    clips.add(clip.id);
  }
  require(["idle", "active", "complete", "alert", "sleep"].every((id) => clips.has(id)), "本地皮肤缺少五种基础动作");
  require(referenced.size === frameIds.size, "本地皮肤包含未使用的帧");
  return freeze(structuredClone(manifest) as unknown as LocalPetSkinManifestV1);
}

export function parseLocalPetSkinPackage(input: unknown): LocalPetSkinPackage {
  const value = object(input);
  require(typeof value.skinId === "string" && shaPattern.test(value.skinId), "本地皮肤存储标识无效");
  return Object.freeze({ skinId: value.skinId, manifest: parseLocalPetSkinManifest(value.manifest) });
}
export function parseLocalPetSkinState(input: unknown): LocalPetSkinState {
  const value = object(input);
  require(Array.isArray(value.skins) && value.skins.length <= LOCAL_PET_SKIN_LIMITS.installed, "本地皮肤列表无效");
  const skins = value.skins.map(parseLocalPetSkinPackage);
  const ids = new Set(skins.map((skin) => skin.skinId));
  require(ids.size === skins.length && (value.selectedSkinId === null ||
    typeof value.selectedSkinId === "string" && ids.has(value.selectedSkinId)), "本地皮肤选择不存在或列表重复");
  require(value.problems === undefined || Array.isArray(value.problems) && value.problems.length <= 32, "本地皮肤诊断无效");
  const problems = ((value.problems ?? []) as unknown[]).map((input) => {
    const problem = object(input);
    require(problem.skinId === null || typeof problem.skinId === "string" && shaPattern.test(problem.skinId), "本地皮肤诊断标识无效");
    text(problem.message, 2048, "皮肤诊断");
    return Object.freeze({ skinId: problem.skinId as string | null, message: problem.message });
  });
  return Object.freeze({ selectedSkinId: value.selectedSkinId as string | null, skins: Object.freeze(skins), problems: Object.freeze(problems) });
}

/** Only fixed IPC commands and opaque identifiers cross this adapter. */
export function createLocalPetSkinClient(invoke: LocalPetSkinInvoke,
  subscribe?: LocalPetSkinClient["subscribe"]): LocalPetSkinClient {
  return {
    async getState() { return parseLocalPetSkinState(await invoke("get_pet_skin_state")); },
    async chooseImport() {
      const result = await invoke("choose_import_pet_skin");
      return result === null ? null : parseLocalPetSkinPackage(result);
    },
    async select(skinId) {
      require(skinId === null || shaPattern.test(skinId), "本地皮肤存储标识无效");
      const state = parseLocalPetSkinState(await invoke("select_pet_skin", { skinId }));
      require(state.selectedSkinId === skinId, "本地皮肤选择响应不一致");
      return state;
    },
    async remove(skinId) {
      require(shaPattern.test(skinId), "本地皮肤存储标识无效");
      return parseLocalPetSkinState(await invoke("remove_pet_skin", { skinId }));
    },
    async readFrame(skinId, frameId) {
      require(shaPattern.test(skinId) && idPattern.test(frameId), "本地皮肤帧请求无效");
      const result = await invoke<ArrayBuffer>("read_pet_skin_frame", { skinId, frameId });
      require(result instanceof ArrayBuffer && result.byteLength >= 45 && result.byteLength <= LOCAL_PET_SKIN_LIMITS.frameBytes, "本地皮肤帧响应无效");
      return result;
    },
    ...(subscribe ? { subscribe } : {}),
  };
}
