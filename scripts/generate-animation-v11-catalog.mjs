import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";

const root = new URL("../", import.meta.url);
const base = "assets/pets/nekomi/";
const pack = `${base}walk-v10-complete/`;
const input = `${pack}catalog-v11.json`;
const output = `${base}manifest-animation-v11.json`;
const urlsOutput = "src/features/pet/builtinAnimationSources.ts";
const read = (file) => readFile(new URL(file, root));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const inputBytes = await read(input);
const source = JSON.parse(inputBytes.toString("utf8"));
const delivery = JSON.parse(await read(`${pack}delivery-manifest-v11.json`));
const old = JSON.parse(await read(`${base}manifest-hd-v1.json`));
assert.equal(source.schemaVersion, 1);
assert.deepEqual(source.canvas, [1024, 1536]);
const ids = ["idle", "active", "alert", "complete", "happy", "wink", "affection", "shy", "bow", "stretch", "hug-tail", "sleep", "walk"];
assert.deepEqual(source.clips.map((clip) => clip.id), ids);
const delivered = new Map(delivery.items.map((frame) => [`${base}${frame.path}`, frame]));
const frames = new Map();
const resolve = (file) => {
  assert.match(file, /^(?:\.\.\/animation-v6\/clips\/[a-z-]+\/|(?:model-reference|actions-v11\/[a-z-]+|frames-v11)\/)[a-z0-9-]+\.png$/);
  const resolved = path.posix.normalize(`${pack}${file}`);
  assert.ok(resolved.startsWith(base));
  return resolved;
};
const frameId = (file) => file.slice(base.length, -4);
for (const clip of source.clips) {
  for (const step of clip.frames) {
    const file = resolve(step.path), frame = delivered.get(file);
    assert.ok(step.exists && frame, `Undelivered frame: ${file}`);
    assert.equal(step.sha256, frame.sha256);
    assert.deepEqual(frame.canvas, [1024, 1536]);
    assert.equal(frame.mode, "RGBA");
    if (frames.has(file)) continue;
    const bytes = await read(file);
    assert.equal(bytes.length, frame.bytes, file);
    assert.equal(hash(bytes), frame.sha256, file);
    assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
    assert.equal(bytes.toString("ascii", 12, 16), "IHDR");
    assert.equal(bytes.readUInt32BE(16), 1024);
    assert.equal(bytes.readUInt32BE(20), 1536);
    assert.deepEqual([...bytes.subarray(24, 29)], [8, 6, 0, 0, 0]);
    const bounds = frame.visibleBounds127;
    assert.ok(bounds.length === 4 && bounds.every(Number.isSafeInteger));
    assert.ok(bounds[0] >= 0 && bounds[1] >= 0 && bounds[2] > bounds[0] && bounds[3] > bounds[1] && bounds[2] <= 1024 && bounds[3] <= 1536);
    frames.set(file, { id: frameId(file), path: file, width: 1024, height: 1536,
      bytes: bytes.length, sha256: frame.sha256, bounds });
  }
}
assert.equal(frames.size, 52);
const front = frameId(`${pack}model-reference/master-front-v2.png`);
const clips = source.clips.map((clip) => {
  const loop = ["idle", "active", "alert"].includes(clip.id);
  const sequence = clip.frames.map((step) => {
    assert.ok(Number.isSafeInteger(step.durationMs) && step.durationMs >= 16 && step.durationMs <= 10000);
    assert.ok(["entry", "body", "exit", "returnToIdle"].includes(step.segment));
    return { frame: frameId(resolve(step.path)), durationMs: step.durationMs, segment: step.segment };
  });
  const start = sequence.findIndex((step) => step.segment === "body");
  const end = sequence.findIndex((step, i) => i > start && step.segment !== "body");
  assert.ok(start >= 0 && end > start);
  assert.ok(sequence.slice(start, end).every((step) => step.segment === "body"));
  const fallback = clip.id === "sleep" ? frameId(`${pack}actions-v11/sleep/seated.png`) :
    ["active", "alert"].includes(clip.id) ? `animation-v6/clips/${clip.id}/base` : front;
  assert.ok([...frames.values()].some((frame) => frame.id === fallback));
  assert.ok(new Set([fallback, ...sequence.map((step) => step.frame)]).size <= 12);
  return { id: clip.id, fallback, canvas: [1024, 1536],
    sharedTransform: { scale: 1, anchor: [512, 1490], translate: [0, 0] },
    loop, ...(loop ? { loopRange: { start, end } } : {}), sequence };
});
const generation = [];
for (const file of [`${pack}delivery-manifest-v11.json`, `${pack}generation-manifest-v2.json`,
  `${base}animation-v6/generation-root.json`, `${base}animation-v6/generation-cute.json`, `${pack}DELIVERY-20261018.txt`,
  ...["master", "walk-key", "walk-fill", "actions-a", "actions-b", "actions-c", "actions-d", "actions-e"]
    .map((name) => `output/imagegen/completion-20261018/source-records-${name}.json`)]) {
  generation.push({ path: file, sha256: hash(await read(file)) });
}
const catalog = {
  schema_version: 2, skin_id: "nekomi-animation-v11", renderer_kind: "builtin-animation-clips", bounds_alpha_threshold: 127,
  author: "Token 喵项目设计 · OpenAI imagegen AI 辅助生成",
  source: "AI-assisted，2026-10-18；v11 初版交付的52张原生透明PNG、12动作与8相位原地侧走演示。保留v6的部分动作主体；原图字节不变，分层及未选稿不进入运行包。保留服装/比例小幅差异与离散关键帧节奏；不构成Native无滑步或左右游走验收。",
  license: "LicenseRef-Project-Owned", attribution: "小柚 · v11 动作素材 · AI 辅助生成内置资源",
  source_archive: { manifest: { path: input, sha256: hash(inputBytes) }, generation,
    base_pack: pack, review_archive: "output/imagegen/completion-20261018/" },
  variants: { idle: [...old.variants.idle, 4, 13], tap: old.variants.tap },
  actions: old.actions, static_pose: old.static_pose,
  frames: [...frames.values()], clips,
};
const content = `${JSON.stringify(catalog, null, 2)}\n`;
// Explicit build-time imports expose only the selected PNGs, including retained
// v6 frames. No broad directory glob can ship layers or rejected candidates.
const entries = [...frames.values()];
const urls = "// Generated by scripts/generate-animation-v11-catalog.mjs. Do not edit.\n" +
  entries.map((frame, i) => `import frame${i} from ${JSON.stringify(`../../../${frame.path}?url`)};`).join("\n") +
  "\n\nexport const bundledAnimationSources: Record<string, string> = Object.freeze({\n" +
  entries.map((frame, i) => `  ${JSON.stringify(frame.path)}: frame${i},`).join("\n") + "\n});\n";
const args = process.argv.slice(2);
assert.ok(args.length === 0 || (args.length === 1 && args[0] === "--check"));
for (const [file, text] of [[output, content], [urlsOutput, urls]]) {
  if (args[0] === "--check") assert.equal(await readFile(new URL(file, root), "utf8"), text, file);
  else await writeFile(new URL(file, root), text, "utf8");
}
console.log(JSON.stringify({ frames: frames.size, clips: clips.length,
  encodedBytes: entries.reduce((sum, frame) => sum + frame.bytes, 0),
  maxClipFrames: Math.max(...clips.map((clip) => new Set([clip.fallback, ...clip.sequence.map((step) => step.frame)]).size)),
  check: args[0] === "--check", sha256: hash(content) }));
