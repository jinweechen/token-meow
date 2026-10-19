import manifestData from "../../../assets/pets/nekomi/manifest-animation-v12.json";
import { bundledAnimationSources } from "./builtinAnimationSources";

export type AnimationClipId = "idle" | "active" | "complete" | "alert" | "sleep" |
  "shy" | "affection" | "stretch" | "wink" | "happy" | "bow" | "hug-tail" | "walk" |
  "walk-right" | "walk-left" | "walk-left-to-right" | "walk-right-to-left";
type Action = "idle" | "active" | "complete" | "alert" | "sleep";
export interface AnimationFrameDefinition {
  id: string;
  path: string;
  width: number;
  height: number;
  bytes: number;
  sha256: string;
  bounds: [number, number, number, number];
}
export interface AnimationClipDefinition {
  id: AnimationClipId;
  fallback: string;
  canvas: [number, number];
  sharedTransform: { scale: number; anchor: [number, number]; translate: [number, number] };
  loop: boolean;
  loopRange?: { start: number; end: number };
  sequence: { frame: string; durationMs: number; segment?: string }[];
}
export interface BuiltinAnimationManifest {
  schema_version: 2;
  skin_id: "nekomi-animation-v6" | "nekomi-animation-v11" | "nekomi-animation-v12";
  bounds_alpha_threshold?: number;
  renderer_kind: "builtin-animation-clips";
  author: string;
  source: string;
  license: string;
  attribution: string;
  source_archive: {
    manifest: { path: string; sha256: string };
    generation: { path: string; sha256: string }[];
    base_pack: string;
    review_archive: string;
  };
  variants: { idle: number[]; tap: number[] };
  actions: Record<Action, {
    start: number; loop: number; end: number; fallback: Action | "static";
    frames: { start: number; loop: number; end: number };
  }>;
  static_pose: AnimationClipId;
  frames: AnimationFrameDefinition[];
  clips: AnimationClipDefinition[];
}

function freeze<T extends object>(value: T): T {
  for (const child of Object.values(value))
    if (child && typeof child === "object") freeze(child);
  return Object.freeze(value);
}

// JSON imports widen tuple and literal types. Catalog generation plus the loader
// validate the raw frame definitions before decoding the immutable originals.
export const builtinAnimationManifest = freeze(
  structuredClone(manifestData) as unknown as BuiltinAnimationManifest,
);
export const builtinAnimationFrames = builtinAnimationManifest.frames;
export const builtinAnimationClips = builtinAnimationManifest.clips;

// Generated explicit ?url imports create strings only, never PNG requests.
// Only the 63 selected whole-character originals can enter the build.
const sources: Record<string, string> = {};
const expected = new Set<string>();
for (const frame of builtinAnimationFrames) {
  const key = frame.path;
  if (key !== `assets/pets/nekomi/${frame.id}.png` ||
      !/^assets\/pets\/nekomi\/(?:animation-v6\/clips\/[a-z-]+\/|walk-v10-complete\/(?:model-reference\/|actions-v11\/[a-z-]+\/|frames-v11\/|actions-v12\/walk-left\/|frames-left-v12\/))[a-z0-9-]+\.png$/.test(key) ||
      expected.has(key) || !Object.hasOwn(bundledAnimationSources, key) ||
      typeof bundledAnimationSources[key] !== "string" || !bundledAnimationSources[key])
    throw Error(`内置动画帧路径不匹配：${frame.id}`);
  expected.add(key);
  sources[frame.id] = bundledAnimationSources[key];
}
if (Object.keys(bundledAnimationSources).length !== expected.size ||
    Object.keys(bundledAnimationSources).some((key) => !expected.has(key)))
  throw Error("内置动画帧清单与打包路径不一致");
export const builtinAnimationSources: Record<string, string> = Object.freeze(sources);
