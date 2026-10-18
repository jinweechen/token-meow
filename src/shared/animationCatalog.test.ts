import assert from "node:assert/strict";
import test from "node:test";
import { readFile, readdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import { inflateSync } from "node:zlib";
import type { BuiltinAnimationManifest } from "../features/pet/builtinAnimationSkin";

const root = new URL("../../", import.meta.url);
const pack = "assets/pets/nekomi/animation-v6/";
const ids = ["idle", "active", "complete", "alert", "sleep", "shy", "affection",
  "stretch", "wink", "happy", "bow", "hug-tail"];
// This handoff hash independently pins the source's frame hashes, order, bounds,
// and clip sequences. A material replacement needs a new reviewed handoff.
const handoffSha = "5a68b8619090fcda9cb512f35e84fa6afde53a9d4ba465ebc0c70079dc91c8ef";
const hash = (bytes: Buffer | string) => createHash("sha256").update(bytes).digest("hex");
const read = (path: string) => readFile(new URL(path, root));
interface HandoffFrame {
  id: string; path: string; width: number; height: number; bytes: number;
  sha256: string; visibleBounds16: [number, number, number, number];
}
interface Handoff {
  frames: HandoffFrame[];
  clips: BuiltinAnimationManifest["clips"];
}
async function fixture() {
  const handoff = await read(`${pack}animation-manifest.json`);
  const catalog = JSON.parse((await read("assets/pets/nekomi/manifest-animation-v6.json")).toString("utf8")) as BuiltinAnimationManifest;
  return { handoff, source: JSON.parse(handoff.toString("utf8")) as Handoff, catalog };
}

// Independently reconstruct original, non-interlaced RGBA rows. The result is
// technical Node evidence, not a browser decode or a Native memory measurement.
function pixels(bytes: Buffer) {
  assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
  assert.equal(bytes.readUInt32BE(8), 13);
  assert.equal(bytes.toString("ascii", 12, 16), "IHDR");
  const width = bytes.readUInt32BE(16), height = bytes.readUInt32BE(20);
  assert.deepEqual([bytes[24], bytes[25], bytes[26], bytes[27], bytes[28]], [8, 6, 0, 0, 0]);
  const chunks: Buffer[] = [];
  let ended = false;
  for (let at = 8; at < bytes.length;) {
    assert.ok(at + 12 <= bytes.length);
    const length = bytes.readUInt32BE(at), end = at + 12 + length;
    assert.ok(end <= bytes.length);
    const kind = bytes.toString("ascii", at + 4, at + 8);
    if (kind === "IDAT") chunks.push(bytes.subarray(at + 8, at + 8 + length));
    if (kind === "IEND") { assert.equal(length, 0); assert.equal(end, bytes.length); ended = true; }
    at = end;
  }
  assert.ok(ended && chunks.length > 0);
  const raw = inflateSync(Buffer.concat(chunks)), stride = width * 4;
  assert.equal(raw.length, (stride + 1) * height);
  let previous = new Uint8Array(stride), row = new Uint8Array(stride);
  let left = width, top = height, right = 0, bottom = 0;
  const corners: number[] = [];
  for (let y = 0; y < height; y++) {
    const at = y * (stride + 1), filter = raw[at];
    assert.ok(filter <= 4);
    for (let x = 0; x < stride; x++) {
      const a = x >= 4 ? row[x - 4] : 0, b = previous[x], c = x >= 4 ? previous[x - 4] : 0;
      let prediction = 0;
      if (filter === 1) prediction = a;
      else if (filter === 2) prediction = b;
      else if (filter === 3) prediction = Math.floor((a + b) / 2);
      else if (filter === 4) {
        const p = a + b - c, da = Math.abs(p - a), db = Math.abs(p - b), dc = Math.abs(p - c);
        prediction = da <= db && da <= dc ? a : db <= dc ? b : c;
      }
      row[x] = (raw[at + 1 + x] + prediction) & 255;
    }
    for (let x = 0; x < width; x++) if (row[x * 4 + 3] >= 16) {
      left = Math.min(left, x); top = Math.min(top, y);
      right = Math.max(right, x + 1); bottom = Math.max(bottom, y + 1);
    }
    if (y === 0 || y === height - 1) corners.push(row[3], row[stride - 1]);
    [row, previous] = [previous, row];
  }
  return { width, height, bounds: [left, top, right, bottom], corners };
}

test("v6 runtime catalog binds the reviewed handoff and excludes separable layers", async () => {
  const { handoff, source, catalog } = await fixture();
  assert.equal(hash(handoff), handoffSha);
  assert.equal(catalog.schema_version, 2);
  assert.equal(catalog.skin_id, "nekomi-animation-v6");
  assert.equal(catalog.renderer_kind, "builtin-animation-clips");
  assert.equal(catalog.license, "LicenseRef-Project-Owned");
  assert.ok(catalog.author.includes("imagegen") && catalog.source.includes("2026-10-17"));
  assert.equal(source.frames.length, 66);
  const selected = source.frames.filter((frame) => frame.path.startsWith("clips/"));
  assert.equal(selected.length, 38);
  assert.deepEqual(catalog.frames, selected.map((frame) => ({ id: frame.id,
    path: `${pack}${frame.path}`, width: frame.width, height: frame.height,
    bytes: frame.bytes, sha256: frame.sha256, bounds: frame.visibleBounds16 })));
  assert.equal(new Set(catalog.frames.map((frame) => frame.id)).size, 38);
  assert.equal(new Set(catalog.frames.map((frame) => frame.path)).size, 38);
  assert.deepEqual(catalog.source_archive.manifest, { path: `${pack}animation-manifest.json`, sha256: handoffSha });
  assert.deepEqual(catalog.source_archive.generation.map((entry) => entry.path),
    [`${pack}generation-root.json`, `${pack}generation-cute.json`]);
  for (const entry of catalog.source_archive.generation) assert.equal(hash(await read(entry.path)), entry.sha256);
  assert.equal(catalog.source_archive.base_pack, "assets/pets/nekomi/hd-v5-natural-tail/");
  assert.equal(catalog.source_archive.review_archive, "output/imagegen/animation-v6-20261017/");
});

test("v6 all 38 originals retain their SHA, RGBA pixels, visible bounds, and v5 bases", async () => {
  const { catalog } = await fixture();
  let total = 0;
  const actualPaths: string[] = [];
  const directories = await readdir(new URL(`${pack}clips/`, root), { withFileTypes: true });
  assert.ok(directories.every((entry) => entry.isDirectory()));
  assert.deepEqual(directories.map((entry) => entry.name).sort(), [...ids].sort());
  for (const id of ids) {
    for (const entry of await readdir(new URL(`${pack}clips/${id}/`, root), { withFileTypes: true })) {
      assert.ok(entry.isFile() && entry.name.endsWith(".png"));
      actualPaths.push(`${pack}clips/${id}/${entry.name}`);
    }
  }
  assert.deepEqual(actualPaths.sort(), catalog.frames.map((frame) => frame.path).sort());
  for (const frame of catalog.frames) {
    assert.equal(frame.path, `${pack}clips/${frame.id}.png`);
    const bytes = await read(frame.path);
    assert.equal(bytes.length, frame.bytes, `${frame.id}: bytes`);
    assert.equal(hash(bytes), frame.sha256, `${frame.id}: SHA`);
    // The reviewed hug-tail accent retains one alpha=1 corner pixel. Pin that
    // original residue explicitly instead of rewriting or silently normalizing it.
    const corners = frame.id === "hug-tail/accent" ? [1, 0, 0, 0] : [0, 0, 0, 0];
    assert.deepEqual(pixels(bytes), { width: 1024, height: 1536,
      bounds: frame.bounds, corners }, `${frame.id}: original pixels`);
    assert.deepEqual([frame.width, frame.height], [1024, 1536]);
    total += bytes.length;
    if (frame.id.endsWith("/base")) {
      const original = await read(`assets/pets/nekomi/hd-v5-natural-tail/${frame.id.split("/")[0]}.png`);
      assert.equal(hash(original), frame.sha256, `${frame.id}: actual v5 original`);
      assert.ok(bytes.equals(original));
    }
  }
  assert.equal(total, 73_937_625);
});

test("v6 twelve clips preserve shared canvas placement and all finite source timings", async () => {
  const { source, catalog } = await fixture();
  assert.deepEqual(catalog.clips.map((clip) => clip.id), ids);
  const used = new Set<string>();
  for (let i = 0; i < catalog.clips.length; i++) {
    const clip = catalog.clips[i], original = source.clips[i];
    assert.deepEqual(clip, { id: original.id, fallback: original.fallback,
      canvas: original.canvas, sharedTransform: original.sharedTransform,
      loop: original.loop, sequence: original.sequence });
    assert.deepEqual(clip.canvas, [1024, 1536]);
    assert.deepEqual(clip.sharedTransform, { scale: 1, anchor: [512, 1490], translate: [0, 0] });
    assert.equal(clip.loop, true);
    assert.equal(clip.fallback, `${clip.id}/base`);
    assert.equal(catalog.frames.filter((frame) => frame.id.startsWith(`${clip.id}/`)).length,
      clip.id === "idle" ? 5 : 3);
    let duration = 0;
    for (const step of clip.sequence) {
      assert.deepEqual(Object.keys(step).sort(), ["durationMs", "frame"]);
      assert.ok(catalog.frames.some((frame) => frame.id === step.frame));
      assert.ok(step.frame.startsWith(`${clip.id}/`));
      assert.ok(Number.isSafeInteger(step.durationMs) && step.durationMs >= 75 && step.durationMs <= 1200);
      used.add(step.frame); duration += step.durationMs;
    }
    assert.ok(duration > 0 && duration <= 8000);
  }
  assert.deepEqual([...used].sort(), catalog.frames.map((frame) => frame.id).sort());
  assert.deepEqual(catalog.clips[0].sequence.slice(5, 8), [
    { frame: "idle/half-blink", durationMs: 75 },
    { frame: "idle/closed-blink", durationMs: 100 },
    { frame: "idle/half-blink", durationMs: 75 },
  ]);
});
