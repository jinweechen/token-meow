import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { inflateSync } from "node:zlib";
import path from "node:path";
import type { BuiltinAnimationManifest } from "../features/pet/builtinAnimationSkin";

const root = new URL("../../", import.meta.url);
const base = "assets/pets/nekomi/";
const pack = `${base}walk-v10-complete/`;
const handoffSha = "aaa468e64eaf04510670441202edfcf2d6c1ad5ca59e9bbb6a5315125a7e7744";
const deliverySha = "64a73c96f27f6b4ed99dc9276f36faeb9d11726e3dec03ad6987c20434b21775";
const ids = ["idle", "active", "alert", "complete", "happy", "wink", "affection",
  "shy", "bow", "stretch", "hug-tail", "sleep", "walk"];
const front = "walk-v10-complete/model-reference/master-front-v2";
const hash = (bytes: Buffer | string) => createHash("sha256").update(bytes).digest("hex");
const read = (file: string) => readFile(new URL(file, root));
const resolveFrame = (file: string) => path.posix.normalize(`${pack}${file}`);
const frameId = (file: string) => file.slice(base.length, -4);
interface SourceStep { path: string; durationMs: number; segment: string; sha256: string; exists: boolean }
interface SourceCatalog { schemaVersion: number; canvas: number[]; clips: { id: string; frames: SourceStep[] }[] }
interface DeliveredFrame {
  path: string; sha256: string; bytes: number; canvas: number[]; mode: string;
  visibleBounds127: number[];
}

async function fixture() {
  const [handoff, deliveryBytes, runtime] = await Promise.all([
    read(`${pack}catalog-v11.json`), read(`${pack}delivery-manifest-v11.json`),
    read(`${base}manifest-animation-v11.json`),
  ]);
  const source = JSON.parse(handoff.toString("utf8")) as SourceCatalog;
  const delivery = JSON.parse(deliveryBytes.toString("utf8")) as { items: DeliveredFrame[] };
  const catalog = JSON.parse(runtime.toString("utf8")) as BuiltinAnimationManifest;
  return { handoff, deliveryBytes, source, delivery, catalog };
}

// Independently reconstruct the unchanged non-interlaced RGBA pixels. Bounds
// deliberately use the v11 delivery's alpha >=127 convention, not v6's >=16.
function pixels(bytes: Buffer) {
  assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
  assert.equal(bytes.readUInt32BE(8), 13);
  assert.equal(bytes.toString("ascii", 12, 16), "IHDR");
  const width = bytes.readUInt32BE(16), height = bytes.readUInt32BE(20);
  assert.deepEqual([...bytes.subarray(24, 29)], [8, 6, 0, 0, 0]);
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
    for (let x = 0; x < width; x++) if (row[x * 4 + 3] >= 127) {
      left = Math.min(left, x); top = Math.min(top, y);
      right = Math.max(right, x + 1); bottom = Math.max(bottom, y + 1);
    }
    if (y === 0 || y === height - 1) corners.push(row[3], row[stride - 1]);
    [row, previous] = [previous, row];
  }
  return { width, height, bounds: [left, top, right, bottom], corners };
}

test("v11 runtime binds the delivered source catalog and only its 52 selected whole-character PNGs", async () => {
  const { handoff, deliveryBytes, source, delivery, catalog } = await fixture();
  assert.equal(hash(handoff), handoffSha);
  assert.equal(hash(deliveryBytes), deliverySha);
  assert.equal(source.schemaVersion, 1);
  assert.deepEqual(source.canvas, [1024, 1536]);
  assert.equal(catalog.schema_version, 2);
  assert.equal(catalog.skin_id, "nekomi-animation-v11");
  assert.equal(catalog.renderer_kind, "builtin-animation-clips");
  assert.equal(catalog.bounds_alpha_threshold, 127);
  assert.equal(catalog.license, "LicenseRef-Project-Owned");
  assert.ok(catalog.author.includes("imagegen") && catalog.source.includes("2026-10-18"));
  assert.deepEqual(catalog.source_archive.manifest, { path: `${pack}catalog-v11.json`, sha256: handoffSha });
  for (const record of catalog.source_archive.generation) assert.equal(hash(await read(record.path)), record.sha256);
  assert.ok(catalog.source_archive.generation.some((record) =>
    record.path === `${pack}delivery-manifest-v11.json` && record.sha256 === deliverySha));
  const selected = [...new Set(source.clips.flatMap((clip) => clip.frames.map((step) => resolveFrame(step.path))))];
  assert.equal(selected.length, 52);
  const delivered = new Map(delivery.items.map((frame) => [`${base}${frame.path}`, frame]));
  assert.deepEqual(catalog.frames, selected.map((file) => {
    const frame = delivered.get(file)!;
    assert.ok(frame, file);
    assert.deepEqual(frame.canvas, [1024, 1536]);
    assert.equal(frame.mode, "RGBA");
    return { id: frameId(file), path: file, width: 1024, height: 1536,
      bytes: frame.bytes, sha256: frame.sha256, bounds: frame.visibleBounds127 };
  }));
  assert.equal(new Set(catalog.frames.map((frame) => frame.id)).size, 52);
  assert.equal(new Set(catalog.frames.map((frame) => frame.path)).size, 52);
  assert.ok(catalog.frames.every((frame) => !/layers|rejected|study/.test(frame.path)));
});

test("v11 all 52 original PNGs retain SHA, actual RGBA dimensions, bounds127 and original alpha corners", async () => {
  const { catalog } = await fixture();
  let total = 0;
  const cornerResidue: Record<string, number[]> = {
    "walk-v10-complete/actions-v11/stretch/enter": [0, 0, 1, 0],
    "walk-v10-complete/frames-v11/07-near-passing": [1, 0, 0, 0],
  };
  for (const frame of catalog.frames) {
    const bytes = await read(frame.path);
    assert.equal(bytes.length, frame.bytes, `${frame.id}: bytes`);
    assert.equal(hash(bytes), frame.sha256, `${frame.id}: SHA`);
    assert.deepEqual(pixels(bytes), { width: 1024, height: 1536, bounds: frame.bounds,
      corners: cornerResidue[frame.id] ?? [0, 0, 0, 0] }, `${frame.id}: original pixels`);
    assert.deepEqual([frame.width, frame.height], [1024, 1536]);
    assert.equal(frame.path, `${base}${frame.id}.png`);
    total += bytes.length;
  }
  assert.equal(total, 100_633_266);
});

test("v11 preserves every catalog step and timing, body-only loops, finite returns and static sleep fallback", async () => {
  const { source, catalog } = await fixture();
  assert.deepEqual(source.clips.map((clip) => clip.id), ids);
  assert.deepEqual(catalog.clips.map((clip) => clip.id), ids);
  const durations = [8020, 3100, 3100, 3100, 3100, 3100, 3100, 3100, 2750, 3300, 3950, 11450, 5670];
  const used = new Set<string>();
  for (let i = 0; i < ids.length; i++) {
    const clip = catalog.clips[i], original = source.clips[i];
    assert.deepEqual(clip.canvas, [1024, 1536]);
    assert.deepEqual(clip.sharedTransform, { scale: 1, anchor: [512, 1490], translate: [0, 0] });
    assert.deepEqual(clip.sequence, original.frames.map((step) => {
      assert.ok(step.exists);
      const id = frameId(resolveFrame(step.path));
      assert.equal(catalog.frames.find((frame) => frame.id === id)?.sha256, step.sha256);
      used.add(id);
      return { frame: id, durationMs: step.durationMs, segment: step.segment };
    }));
    assert.equal(clip.sequence.reduce((sum, step) => sum + step.durationMs, 0), durations[i]);
    assert.deepEqual([...new Set(clip.sequence.map((step) => step.segment))], ["entry", "body", "exit", "returnToIdle"]);
    const loop = ["idle", "active", "alert"].includes(clip.id);
    assert.equal(clip.loop, loop);
    assert.deepEqual(clip.loopRange, loop ? { start: clip.id === "idle" ? 1 : 2, end: clip.id === "idle" ? 8 : 6 } : undefined);
    if (clip.loopRange) assert.ok(clip.sequence.slice(clip.loopRange.start, clip.loopRange.end)
      .every((step) => step.segment === "body"));
    assert.equal(clip.fallback, clip.id === "sleep" ? "walk-v10-complete/actions-v11/sleep/seated" :
      ["active", "alert"].includes(clip.id) ? `animation-v6/clips/${clip.id}/base` : front);
    assert.ok(catalog.frames.some((frame) => frame.id === clip.fallback));
    assert.ok(new Set([clip.fallback, ...clip.sequence.map((step) => step.frame)]).size <= 12);
  }
  assert.deepEqual([...used].sort(), catalog.frames.map((frame) => frame.id).sort());
  const walk = catalog.clips.find((clip) => clip.id === "walk")!;
  assert.equal(walk.sequence.length, 26);
  assert.equal(new Set(walk.sequence.map((step) => step.frame)).size, 12);
  const body = walk.sequence.filter((step) => step.segment === "body");
  assert.equal(body.length, 16);
  assert.deepEqual(body.slice(0, 8), body.slice(8));
  assert.ok(body.every((step) => step.durationMs === 160 && step.frame.startsWith("walk-v10-complete/frames-v11/")));
  assert.ok(walk.sequence.filter((step) => step.frame.endsWith("master-right-v1"))
    .every((step) => step.segment === "entry" || step.segment === "exit"));
});
