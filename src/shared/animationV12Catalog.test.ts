import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { inflateSync } from "node:zlib";
import path from "node:path";
import ts from "typescript";
import type { BuiltinAnimationManifest } from "../features/pet/builtinAnimationSkin";
import { HD_LOGICAL_POSES } from "../features/pet/hdPoseLayout";

const root = new URL("../../", import.meta.url);
const base = "assets/pets/nekomi/";
const pack = `${base}walk-v10-complete/`;
const handoffSha = "731d2586c16fe2fec25f5004b93b33c4ed4bcb09857e42d88fb28bc54f7fa559";
const deliverySha = "287ecaea9b2dc8399d8a56dfbbb1740c3f85bb81ed6c9b975d911accc3c62701";
const ids = ["idle", "active", "alert", "complete", "happy", "wink", "affection",
  "shy", "bow", "stretch", "hug-tail", "sleep", "walk-right", "walk-left", "walk-left-to-right", "walk-right-to-left"];
const front = "walk-v10-complete/model-reference/master-front-v2";
const hash = (bytes: Buffer | string) => createHash("sha256").update(bytes).digest("hex");
const read = (file: string) => readFile(new URL(file, root));
const resolveFrame = (file: string) => path.posix.normalize(`${pack}${file}`);
const frameId = (file: string) => file.slice(base.length, -4);
interface SourceStep { path: string; durationMs: number; segment: string; sha256: string; exists: boolean }
interface SourceCatalog { schemaVersion: number; canvas: number[]; clips: { id: string; frames: SourceStep[] }[] }
interface DeliveredFrame {
  path: string; sha256: string; bytes: number; canvas: number[]; mode: string;
  visibleBounds127: number[]; alphaRange: number[];
}

async function fixture() {
  const [handoff, deliveryBytes, runtime] = await Promise.all([
    read(`${pack}catalog-v12.json`), read(`${pack}delivery-manifest-v12.json`),
    read(`${base}manifest-animation-v12.json`),
  ]);
  const source = JSON.parse(handoff.toString("utf8")) as SourceCatalog;
  const delivery = JSON.parse(deliveryBytes.toString("utf8")) as { items: DeliveredFrame[] };
  const catalog = JSON.parse(runtime.toString("utf8")) as BuiltinAnimationManifest;
  return { handoff, deliveryBytes, source, delivery, catalog };
}

// Independently reconstruct the unchanged non-interlaced RGBA pixels. Bounds
// deliberately use the v12 delivery's alpha >=127 convention, not v6's >=16.
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
  let left = width, top = height, right = 0, bottom = 0, alphaMin = 255, alphaMax = 0;
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
    for (let x = 0; x < width; x++) {
      const alpha = row[x * 4 + 3];
      alphaMin = Math.min(alphaMin, alpha); alphaMax = Math.max(alphaMax, alpha);
      if (alpha >= 127) {
        left = Math.min(left, x); top = Math.min(top, y);
        right = Math.max(right, x + 1); bottom = Math.max(bottom, y + 1);
      }
    }
    if (y === 0 || y === height - 1) corners.push(row[3], row[stride - 1]);
    [row, previous] = [previous, row];
  }
  return { width, height, bounds: [left, top, right, bottom], corners, alphaRange: [alphaMin, alphaMax] };
}

test("v12 runtime binds the delivered source catalog and only its 63 selected whole-character PNGs", async () => {
  const { handoff, deliveryBytes, source, delivery, catalog } = await fixture();
  assert.equal(hash(handoff), handoffSha);
  assert.equal(hash(deliveryBytes), deliverySha);
  assert.equal(source.schemaVersion, 1);
  assert.deepEqual(source.canvas, [1024, 1536]);
  assert.equal(catalog.schema_version, 2);
  assert.equal(catalog.skin_id, "nekomi-animation-v12");
  assert.equal(catalog.renderer_kind, "builtin-animation-clips");
  assert.equal(catalog.bounds_alpha_threshold, 127);
  assert.equal(catalog.license, "LicenseRef-Project-Owned");
  assert.ok(catalog.author.includes("imagegen") && catalog.source.includes("2026-10-18"));
  assert.deepEqual(catalog.source_archive.manifest, { path: `${pack}catalog-v12.json`, sha256: handoffSha });
  for (const record of catalog.source_archive.generation) assert.equal(hash(await read(record.path)), record.sha256);
  assert.ok(catalog.source_archive.generation.some((record) =>
    record.path === `${pack}delivery-manifest-v12.json` && record.sha256 === deliverySha));
  const selected = [...new Set(source.clips.flatMap((clip) => clip.frames.map((step) => resolveFrame(step.path))))];
  assert.equal(selected.length, 63);
  const delivered = new Map(delivery.items.map((frame) => [`${base}${frame.path}`, frame]));
  assert.deepEqual(catalog.frames, selected.map((file) => {
    const frame = delivered.get(file)!;
    assert.ok(frame, file);
    assert.deepEqual(frame.canvas, [1024, 1536]);
    assert.equal(frame.mode, "RGBA");
    return { id: frameId(file), path: file, width: 1024, height: 1536,
      bytes: frame.bytes, sha256: frame.sha256, bounds: frame.visibleBounds127 };
  }));
  assert.equal(new Set(catalog.frames.map((frame) => frame.id)).size, 63);
  assert.equal(new Set(catalog.frames.map((frame) => frame.path)).size, 63);
  assert.ok(catalog.frames.every((frame) => !/layers|rejected|study/.test(frame.path)));
});

test("v12 all 63 original PNGs retain SHA, actual RGBA dimensions, bounds127 and original alpha corners", async () => {
  const { catalog, delivery } = await fixture();
  const delivered = new Map(delivery.items.map((frame) => [`${base}${frame.path}`, frame]));
  let total = 0;
  const cornerResidue: Record<string, number[]> = {
    "walk-v10-complete/actions-v11/stretch/enter": [0, 0, 1, 0],
    "walk-v10-complete/frames-v11/07-near-passing": [1, 0, 0, 0],
    // Pin this native alpha=1 residue; preserve its PNG bytes exactly.
    "walk-v10-complete/frames-left-v12/02-near-flat-recovery": [0, 0, 1, 0],
  };
  for (const frame of catalog.frames) {
    const bytes = await read(frame.path);
    assert.equal(bytes.length, frame.bytes, `${frame.id}: bytes`);
    assert.equal(hash(bytes), frame.sha256, `${frame.id}: SHA`);
    assert.deepEqual(pixels(bytes), { width: 1024, height: 1536, bounds: frame.bounds,
      corners: cornerResidue[frame.id] ?? [0, 0, 0, 0], alphaRange: delivered.get(frame.path)!.alphaRange }, `${frame.id}: original pixels`);
    assert.deepEqual([frame.width, frame.height], [1024, 1536]);
    assert.equal(frame.path, `${base}${frame.id}.png`);
    total += bytes.length;
  }
  assert.equal(total, 121_997_289);
  assert.ok(catalog.frames.length <= 64 && total <= 128 * 1024 * 1024);
});

test("v12 retains v11 steps and independently binds every new native PNG, prompt and ordered input", async () => {
  const { source, catalog } = await fixture();
  const old = JSON.parse((await read(`${base}manifest-animation-v11.json`)).toString("utf8")) as BuiltinAnimationManifest;
  const previousSource = JSON.parse((await read(`${pack}catalog-v11.json`)).toString("utf8")) as SourceCatalog;
  const records = JSON.parse((await read("output/imagegen/left-completion-20261018/source-records.json")).toString("utf8")) as {
    id: string; source: string; destination: string; prompt: string; references: string[];
    transparent_background: boolean; generation: string;
  }[];
  const integrity = JSON.parse((await read("output/imagegen/left-completion-20261018/integrity.json")).toString("utf8")) as (typeof records[number] & {
    bytesEqual: boolean; sha256: string; originalSha256: string; promptSha256: string;
    inputs: { path: string; sha256: string }[];
  })[];
  for (const clip of previousSource.clips) assert.deepEqual(
    source.clips.find((candidate) => candidate.id === (clip.id === "walk" ? "walk-right" : clip.id))!.frames, clip.frames);
  for (const frame of old.frames) assert.deepEqual(catalog.frames.find((candidate) => candidate.path === frame.path), frame);
  for (const entry of old.source_archive.generation) assert.ok(catalog.source_archive.generation.some((record) =>
    record.path === entry.path && record.sha256 === entry.sha256));
  const oldPaths = new Set(old.frames.map((frame) => frame.path));
  const added = catalog.frames.filter((frame) => !oldPaths.has(frame.path));
  assert.equal(added.length, 11);
  assert.equal(records.length, 11);
  assert.equal(integrity.length, 11);
  assert.deepEqual(added.map((frame) => frame.path).sort(), records.map((record) => resolveFrame(record.destination)).sort());
  assert.equal(catalog.source_archive.generation.length, 33);
  assert.deepEqual(catalog.source_archive.generation.filter((record) => record.path.startsWith("output/")).map((record) => record.path), [
    ...["master", "walk-key", "walk-fill", "actions-a", "actions-b", "actions-c", "actions-d", "actions-e"]
      .map((name) => `output/imagegen/completion-20261018/source-records-${name}.json`),
    ...["source-records.json", "integrity.json", "review-summary.json"].map((name) => `output/imagegen/left-completion-20261018/${name}`),
  ]);
  for (const record of records) {
    const checked = integrity.find((entry) => entry.destination === record.destination)!;
    assert.equal(record.generation, "built-in imagegen");
    assert.equal(record.transparent_background, true);
    assert.ok(record.source.length > 0);
    assert.equal(checked.bytesEqual, true);
    assert.equal(checked.sha256, checked.originalSha256);
    assert.equal(catalog.frames.find((frame) => frame.path === resolveFrame(record.destination))?.sha256, checked.sha256);
    assert.deepEqual(checked.inputs.map((entry) => entry.path), record.references);
    assert.equal(hash(await read(`${pack}${record.prompt}`)), checked.promptSha256);
    assert.ok(catalog.source_archive.generation.some((entry) =>
      entry.path === `${pack}${record.prompt}` && entry.sha256 === checked.promptSha256));
    for (const entry of checked.inputs) assert.equal(hash(await read(resolveFrame(entry.path))), entry.sha256);
  }
  assert.match(catalog.source, /容差/);
  assert.match(catalog.source, /不授予Native移动权限/);
  assert.equal(catalog.source_archive.review_archive, "output/imagegen/left-completion-20261018/");
});

test("v12 preserves every catalog step and timing, body-only loops, finite returns and static sleep fallback", async () => {
  const { source, catalog } = await fixture();
  assert.deepEqual(source.clips.map((clip) => clip.id), ids);
  assert.deepEqual(catalog.clips.map((clip) => clip.id), ids);
  const durations = [8020, 3100, 3100, 3100, 3100, 3100, 3100, 3100, 2750, 3300, 3950, 11450, 5670, 5670, 6560, 6560];
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
    assert.equal(new Set([clip.fallback, ...clip.sequence.map((step) => step.frame)]).size,
      clip.id.includes("-to-") ? 23 : new Set(original.frames.map((step) => step.path)).size);
  }
  assert.deepEqual([...used].sort(), catalog.frames.map((frame) => frame.id).sort());
  const walk = catalog.clips.find((clip) => clip.id === "walk-right")!;
  assert.equal(walk.sequence.length, 26);
  assert.equal(new Set(walk.sequence.map((step) => step.frame)).size, 12);
  const body = walk.sequence.filter((step) => step.segment === "body");
  assert.equal(body.length, 16);
  assert.deepEqual(body.slice(0, 8), body.slice(8));
  assert.ok(body.every((step) => step.durationMs === 160 && step.frame.startsWith("walk-v10-complete/frames-v11/")));
  assert.ok(walk.sequence.filter((step) => step.frame.endsWith("master-right-v1"))
    .every((step) => step.segment === "entry" || step.segment === "exit"));
  const leftWalk = catalog.clips.find((clip) => clip.id === "walk-left")!;
  const leftBody = leftWalk.sequence.filter((step) => step.segment === "body");
  assert.deepEqual(leftBody.slice(0, 8), leftBody.slice(8));
  assert.ok(leftBody.every((step) => step.durationMs === 160 && step.frame.startsWith("walk-v10-complete/frames-left-v12/")));
  for (const id of ["walk-left-to-right", "walk-right-to-left"]) {
    const turn = catalog.clips.find((clip) => clip.id === id)!;
    assert.equal(turn.sequence.length, 32);
    assert.equal(new Set(turn.sequence.map((step) => step.frame)).size, 23);
    assert.equal(turn.sequence.at(-1)?.segment, "returnToIdle");
    assert.equal(turn.loop, false);
    assert.equal(turn.sequence.at(-1)?.frame, front);
  }
});

function compile(source: string) {
  return ts.transpileModule(source, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, esModuleInterop: true,
  } }).outputText;
}

async function moduleFixture() {
  const { catalog } = await fixture();
  const [skinText, sourcesText] = await Promise.all([
    read("src/features/pet/builtinAnimationSkin.ts"), read("src/features/pet/builtinAnimationSources.ts"),
  ]);
  const sourceAst = ts.createSourceFile("builtinAnimationSources.ts", sourcesText.toString("utf8"), ts.ScriptTarget.ES2022, true);
  const sourceImports = sourceAst.statements.filter(ts.isImportDeclaration);
  assert.equal(sourceImports.length, 63);
  assert.deepEqual(sourceImports.map((item) => (item.moduleSpecifier as ts.StringLiteral).text),
    catalog.frames.map((frame) => `../../../${frame.path}?url`));
  assert.ok(sourceImports.every((item) => item.importClause?.name && !item.importClause.namedBindings));
  const calls: string[] = [];
  const bundled: Record<string, unknown> = {};
  new Function("exports", "require", compile(sourcesText.toString("utf8")))(bundled, (file: string) => {
    calls.push(file);
    assert.ok(file.startsWith("../../../assets/pets/nekomi/") && file.endsWith(".png?url"));
    return { __esModule: true, default: `https://pet.test/${file.slice(9, -4)}` };
  });
  assert.equal(calls.length, 63);
  const sources = bundled.bundledAnimationSources as Record<string, string>;
  assert.ok(Object.isFrozen(sources));
  assert.deepEqual(sources, Object.fromEntries(catalog.frames.map((frame) => [frame.path, `https://pet.test/${frame.path}`])));
  const skinAst = ts.createSourceFile("builtinAnimationSkin.ts", skinText.toString("utf8"), ts.ScriptTarget.ES2022, true);
  assert.deepEqual(skinAst.statements.filter(ts.isImportDeclaration)
    .map((item) => (item.moduleSpecifier as ts.StringLiteral).text),
  ["../../../assets/pets/nekomi/manifest-animation-v12.json", "./builtinAnimationSources"]);
  assert.ok(!skinText.toString("utf8").includes("import.meta.glob"));
  const compiledSkin = compile(skinText.toString("utf8"));
  const bind = (manifest: BuiltinAnimationManifest, urls: Record<string, string>) => {
    const exports: Record<string, unknown> = {};
    new Function("exports", "require", compiledSkin)(exports, (file: string) => {
      if (file === "../../../assets/pets/nekomi/manifest-animation-v12.json")
        return { __esModule: true, default: manifest };
      assert.equal(file, "./builtinAnimationSources");
      return { bundledAnimationSources: urls };
    });
    return exports;
  };
  return { catalog, sources, bind };
}

test("v12 imports only the 63 selected PNG URLs, preserves semantic indexes and freezes independent runtime objects", async () => {
  const { catalog, sources, bind } = await moduleFixture();
  const old = JSON.parse((await read(`${base}manifest-hd-v1.json`)).toString("utf8")) as BuiltinAnimationManifest;
  assert.deepEqual(catalog.actions, old.actions);
  assert.deepEqual(catalog.variants, { idle: [...old.variants.idle, 4, 13, 14, 15, 16], tap: old.variants.tap });
  assert.equal(catalog.static_pose, old.static_pose);
  assert.deepEqual(HD_LOGICAL_POSES.slice(13), ["walk-right", "walk-left", "walk-left-to-right", "walk-right-to-left"]);
  assert.ok(catalog.variants.idle.every((index) => catalog.clips.some((clip) => clip.id === HD_LOGICAL_POSES[index])));
  assert.ok([13, 14, 15, 16].every((index) => !catalog.variants.tap.includes(index)));
  const exports = bind(catalog, sources);
  const manifest = exports.builtinAnimationManifest as BuiltinAnimationManifest;
  assert.equal(exports.builtinAnimationFrames, manifest.frames);
  assert.equal(exports.builtinAnimationClips, manifest.clips);
  assert.deepEqual(exports.builtinAnimationSources, Object.fromEntries(catalog.frames.map((frame) =>
    [frame.id, sources[frame.path]])));
  function frozen(value: object) {
    assert.ok(Object.isFrozen(value));
    for (const child of Object.values(value)) if (child && typeof child === "object") frozen(child);
  }
  frozen(manifest);
  assert.ok(Object.isFrozen(exports.builtinAnimationSources));
  assert.notEqual(manifest, catalog);
  assert.notEqual(manifest.clips[0].sharedTransform, catalog.clips[0].sharedTransform);
  catalog.frames[0].path = "changed-by-fixture";
  assert.equal(manifest.frames[0].path, `${base}${front}.png`);
});

test("v12 URL binding rejects missing, empty, extra, duplicate and redirected frame paths", async () => {
  const { catalog, sources, bind } = await moduleFixture();
  const first = catalog.frames[0].path;
  const missing = { ...sources }; delete missing[first];
  assert.throws(() => bind(catalog, missing), /路径不匹配/);
  assert.throws(() => bind(catalog, { ...sources, [first]: "" }), /路径不匹配/);
  assert.throws(() => bind(catalog, { ...sources,
    [`${pack}frames-v11/unlisted.png`]: "https://pet.test/unlisted.png" }), /路径不一致/);
  const duplicate = structuredClone(catalog); duplicate.frames.push(duplicate.frames[0]);
  assert.throws(() => bind(duplicate, sources), /路径不匹配/);
  const mismatch = structuredClone(catalog); mismatch.frames[0].id = "animation-v6/clips/idle/base";
  assert.throws(() => bind(mismatch, sources), /路径不匹配/);
  for (const redirectedPath of [
    `${base}animation-v6/layers/tail.png`, `${pack}layers-side-v11/head.png`, `${pack}layers-side-v12/head.png`,
    `${pack}actions-v12/idle/unlisted.png`, `${pack}frames-left-v12/../frames-v11/01-contact-near.png`,
    `${pack}../walk-v10-complete/model-reference/master-front-v2.png`,
    "https://external.test/frame.png",
  ]) {
    const redirected = structuredClone(catalog);
    redirected.frames[0].path = redirectedPath;
    redirected.frames[0].id = frameId(redirectedPath);
    assert.throws(() => bind(redirected, { ...sources, [redirectedPath]: "https://pet.test/redirected.png" }), /路径不匹配/);
  }
});
