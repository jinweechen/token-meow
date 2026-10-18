import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile, readdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const root = new URL("../", import.meta.url);
const packPath = "assets/pets/nekomi/animation-v6/";
const manifestPath = `${packPath}animation-manifest.json`;
const outputPath = "assets/pets/nekomi/manifest-animation-v6.json";
const clipIds = ["idle", "active", "complete", "alert", "sleep", "shy",
  "affection", "stretch", "wink", "happy", "bow", "hug-tail"];
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const read = (path) => readFile(new URL(path, root));
const manifestBytes = await read(manifestPath);
const source = JSON.parse(manifestBytes.toString("utf8"));
const oldSemantics = JSON.parse((await read("assets/pets/nekomi/manifest-hd-v1.json")).toString("utf8"));

assert.equal(source.packId, "nekomi-animation-v6");
assert.equal(source.status, "approved");
assert.equal(source.pixelPolicy, "unaltered-native-originals");
assert.deepEqual(source.canvas, [1024, 1536]);
assert.deepEqual(source.clips.map((clip) => clip.id), clipIds);

// The handoff also lists 28 separable layers; only whole-character clip frames
// belong to this renderer. Never derive frame locations from those layer bounds.
const sourceFrames = source.frames.filter((frame) => frame.path.startsWith("clips/"));
assert.equal(sourceFrames.length, 38);
assert.equal(new Set(sourceFrames.map((frame) => frame.id)).size, 38);
const frames = [];
for (const frame of sourceFrames) {
  assert.match(frame.id, /^(?:idle|active|complete|alert|sleep|shy|affection|stretch|wink|happy|bow|hug-tail)\/(?:base|enter|accent|half-blink|closed-blink)$/);
  assert.equal(frame.path, `clips/${frame.id}.png`);
  assert.ok(frame.errors.length === 0 && frame.exists);
  const bytes = await read(`${packPath}${frame.path}`);
  assert.equal(bytes.length, frame.bytes, `${frame.id}: bytes`);
  assert.equal(sha256(bytes), frame.sha256, `${frame.id}: SHA-256`);
  assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
  assert.equal(bytes.toString("ascii", 12, 16), "IHDR");
  assert.equal(bytes.readUInt32BE(16), 1024);
  assert.equal(bytes.readUInt32BE(20), 1536);
  assert.equal(bytes[24], 8);
  assert.equal(bytes[25], 6);
  assert.equal(frame.width, 1024);
  assert.equal(frame.height, 1536);
  assert.equal(frame.visibleBounds16.length, 4);
  const [left, top, right, bottom] = frame.visibleBounds16;
  assert.ok([left, top, right, bottom].every(Number.isSafeInteger));
  assert.ok(left >= 0 && top >= 0 && right > left && bottom > top && right <= 1024 && bottom <= 1536);
  if (frame.id.endsWith("/base")) {
    const pose = frame.id.split("/")[0];
    assert.equal(sha256(await read(`assets/pets/nekomi/hd-v5-natural-tail/${pose}.png`)),
      frame.sha256, `${frame.id}: unchanged v5 base`);
  }
  frames.push({ id: frame.id, path: `${packPath}${frame.path}`, width: frame.width,
    height: frame.height, bytes: frame.bytes, sha256: frame.sha256,
    bounds: frame.visibleBounds16 });
}
// Catch an unlisted PNG before Vite's static glob could expose it at runtime.
const actualPaths = [];
const clipDirectories = await readdir(new URL(`${packPath}clips/`, root), { withFileTypes: true });
assert.ok(clipDirectories.every((entry) => entry.isDirectory()));
assert.deepEqual(clipDirectories.map((entry) => entry.name).sort(), [...clipIds].sort());
for (const clip of clipIds) {
  for (const entry of await readdir(new URL(`${packPath}clips/${clip}/`, root), { withFileTypes: true })) {
    assert.ok(entry.isFile() && entry.name.endsWith(".png"), `unexpected clip entry: ${clip}/${entry.name}`);
    actualPaths.push(`${packPath}clips/${clip}/${entry.name}`);
  }
}
assert.deepEqual(actualPaths.sort(), frames.map((frame) => frame.path).sort());
const frameIds = new Set(frames.map((frame) => frame.id));
const used = new Set();
const clips = source.clips.map((clip) => {
  assert.deepEqual(clip.canvas, [1024, 1536]);
  assert.deepEqual(clip.sharedTransform, { scale: 1, anchor: [512, 1490], translate: [0, 0] });
  assert.equal(clip.loop, true);
  assert.equal(clip.fallback, `${clip.id}/base`);
  assert.ok(frameIds.has(clip.fallback));
  assert.ok(clip.sequence.length > 0 && clip.sequence.length <= 16);
  const sequence = clip.sequence.map(({ frame, durationMs }) => {
    assert.ok(frameIds.has(frame) && frame.startsWith(`${clip.id}/`));
    assert.ok(Number.isSafeInteger(durationMs) && durationMs >= 75 && durationMs <= 1200);
    used.add(frame);
    return { frame, durationMs };
  });
  return { id: clip.id, fallback: clip.fallback, canvas: clip.canvas,
    sharedTransform: clip.sharedTransform, loop: clip.loop, sequence };
});
assert.deepEqual([...used].sort(), [...frameIds].sort());
const generation = [];
for (const path of [`${packPath}generation-root.json`, `${packPath}generation-cute.json`])
  generation.push({ path, sha256: sha256(await read(path)) });
const catalog = {
  schema_version: 2,
  skin_id: "nekomi-animation-v6",
  renderer_kind: "builtin-animation-clips",
  author: "Token 喵项目设计 · OpenAI imagegen AI 辅助生成",
  source: "AI-assisted，2026-10-17；38 张原生透明 PNG、12 个有限局部关键帧动作。12 张 base 为 hd-v5-natural-tail 原图的逐字节复制，新增帧保留生成器原始字节；来源和提示词见 animation-v6/generation-root.json、generation-cute.json 及 prompts/。逐帧技术和视觉复查归档在 output/imagegen/animation-v6-20261017；本清单不包含拆分图层、完整动作间过渡或行走步态。",
  license: "LicenseRef-Project-Owned",
  attribution: "小柚 · v6 局部关键帧动画 · AI 辅助生成内置资源",
  source_archive: {
    manifest: { path: manifestPath, sha256: sha256(manifestBytes) },
    generation,
    base_pack: "assets/pets/nekomi/hd-v5-natural-tail/",
    review_archive: "output/imagegen/animation-v6-20261017/",
  },
  variants: oldSemantics.variants,
  actions: oldSemantics.actions,
  static_pose: oldSemantics.static_pose,
  frames,
  clips,
};
const content = `${JSON.stringify(catalog, null, 2)}\n`;
const args = process.argv.slice(2);
assert.ok(args.length === 0 || (args.length === 1 && args[0] === "--check"), "usage: generate-animation-catalog.mjs [--check]");
if (args[0] === "--check") assert.equal(await readFile(new URL(outputPath, root), "utf8"), content);
else await writeFile(new URL(outputPath, root), content, "utf8");
console.log(JSON.stringify({ path: fileURLToPath(new URL(outputPath, root)), frames: frames.length,
  clips: clips.length, encodedBytes: frames.reduce((sum, frame) => sum + frame.bytes, 0),
  check: args[0] === "--check", sha256: sha256(content) }));
