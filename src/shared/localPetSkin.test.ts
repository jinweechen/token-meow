import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { createLocalPetSkinClient, parseLocalPetSkinManifest, parseLocalPetSkinPackage,
  parseLocalPetSkinState, type LocalPetSkinClient, type LocalPetSkinPackage, type LocalPetSkinState } from "./localPetSkin";
import { createLocalPetSkinRuntime, validateLocalPetSkinFrames } from "../features/pet/localPetSkinRuntime";
import { LocalPetSkinController } from "../features/pet/localPetSkinController";
import { createFrameDisplayGate } from "../features/pet/frameDisplayGate";
import type { AnimationClipHost } from "../features/pet/animationClipRegistry";

// Structural PNG + injected decoder test trust/lifetime rules, not browser pixels.
function png(marker = 0) {
  const bytes = new Uint8Array(58), view = new DataView(bytes.buffer);
  bytes.set([137, 80, 78, 71, 13, 10, 26, 10]); view.setUint32(8, 13);
  bytes.set([73, 72, 68, 82], 12); view.setUint32(16, 1024); view.setUint32(20, 1536);
  bytes.set([8, 6, 0, 0, 0], 24); view.setUint32(33, 1);
  bytes.set([73, 68, 65, 84, marker], 37); bytes.set([73, 69, 78, 68], 50);
  return bytes.buffer;
}
function fixture(): LocalPetSkinPackage {
  return parseLocalPetSkinPackage({ skinId: "a".repeat(64), manifest: {
    schema_version: 1, renderer_kind: "local-png-clips", bounds_alpha_threshold: 127,
    name: "测试角色", author: "作者", source: "本地原创", license: "自有素材", attribution: "作者署名", static_pose: "idle",
    frames: [0, 1].map((i) => ({ id: `frame-${i}`, path: `frames/frame-${i}.png`, width: 1024, height: 1536,
      bytes: 58, sha256: createHash("sha256").update(new Uint8Array(png(i))).digest("hex"), bounds: [12, 20, 1000, 1490] })),
    clips: ["idle", "active", "complete", "alert", "sleep"].map((id) => ({ id, fallback: "frame-0", canvas: [1024, 1536],
      sharedTransform: { scale: 1, anchor: [512, 1490], translate: [0, 0] }, loop: id !== "complete",
      sequence: [{ frame: "frame-0", durationMs: 100 }, { frame: "frame-1", durationMs: 100 }] })),
  } });
}
function mutable() { return structuredClone(fixture().manifest); }
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));
function setup() {
  const pkg = fixture(), selected: (string | null)[] = [], read: string[] = [];
  let state: LocalPetSkinState = { selectedSkinId: null, skins: [pkg] };
  let listener: Parameters<NonNullable<LocalPetSkinClient["subscribe"]>>[0] | undefined;
  const client: LocalPetSkinClient = {
    async getState() { return state; }, async chooseImport() { return pkg; },
    async select(id) { selected.push(id); return state = { ...state, selectedSkinId: id }; },
    async remove(id) { return state = { ...state, skins: state.skins.filter((skin) => skin.skinId !== id) }; },
    async readFrame(id, frame) { assert.equal(id, pkg.skinId); read.push(frame); return png(frame === "frame-1" ? 1 : 0); },
    async subscribe(changed) { listener = changed; return () => { listener = undefined; }; },
  };
  return { pkg, client, selected, read, notify: (kind: "selected" | "imported" | "removed") => listener?.({ kind }) };
}
function imageHost() {
  let decodes = 0, closes = 0, live = 0, maximum = 0, next = 0;
  const urls = new Set<string>();
  const host: AnimationClipHost = {
    async fetch() { throw Error("network must never be called"); },
    async decode() { ++decodes; maximum = Math.max(maximum, ++live); return { width: 1024, height: 1536, corners: [0, 1, 0, 0],
      close() { --live; ++closes; } }; },
    createUrl() { const value = `blob:test-${++next}`; urls.add(value); return value; },
    revokeUrl(value) { assert.ok(urls.delete(value)); },
  };
  return { host, urls, counts: () => ({ decodes, closes, live, maximum }) };
}

test("local PNG schema rejects unknown/gait, unsafe paths/text and incompatible clip contracts", () => {
  assert.equal(Object.isFrozen(fixture().manifest.frames), true);
  for (const change of [
    (m: any) => { m.gait = true; }, (m: any) => { m.frames[0].path = "../outside.png"; },
    (m: any) => { m.frames[0].id = "con"; m.frames[0].path = "frames/con.png"; },
    (m: any) => { m.name = "x\u202eexe"; }, (m: any) => { m.name = " padded "; },
    (m: any) => { m.license = ""; }, (m: any) => { m.bounds_alpha_threshold = 1; },
    (m: any) => { m.clips.pop(); }, (m: any) => { m.clips[0].loop = false; },
    (m: any) => { m.clips[2].loop = true; }, (m: any) => { m.clips[0].sequence[0].durationMs = 15; },
    (m: any) => { m.clips[0].sequence = Array(7).fill({ frame: "frame-0", durationMs: 10000 }); },
    (m: any) => { m.clips[2].loopRange = { start: 0, end: 1 }; },
    (m: any) => { m.frames.push({ ...m.frames[0], id: "unused", path: "frames/unused.png" }); },
  ]) { const manifest = mutable(); change(manifest); assert.throws(() => parseLocalPetSkinManifest(manifest)); }
});

test("IPC uses fixed commands/opaque IDs and verifies exact select response", async () => {
  const { pkg } = setup(), calls: unknown[] = [];
  const client = createLocalPetSkinClient(async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push([command, args]);
    return (command === "read_pet_skin_frame" ? png() : command === "choose_import_pet_skin" ? pkg :
      { selectedSkinId: null, skins: [pkg] }) as T;
  });
  await client.readFrame(pkg.skinId, "frame-0");
  await assert.rejects(client.readFrame(pkg.skinId, "../outside"));
  await assert.rejects(client.select(pkg.skinId), /不一致/);
  assert.deepEqual(calls[0], ["read_pet_skin_frame", { skinId: pkg.skinId, frameId: "frame-0" }]);
  assert.equal(calls.length, 2);
});

test("runtime reads only current clip through opaque IPC, aliases missing poses, and releases URLs", async () => {
  const { pkg, client, read } = setup(), images = imageHost();
  const runtime = createLocalPetSkinRuntime(pkg, client, () => {}, images.host);
  assert.equal(runtime.gaitAvailable, false); assert.equal(runtime.clips.length, 13);
  assert.equal(runtime.clips.find((clip) => clip.id === "wink")!.fallback, "frame-0");
  assert.deepEqual(read, []);
  const registry = runtime.getRegistry();
  await registry.load("wink", true); assert.deepEqual(read, ["frame-0"]);
  await registry.load("active"); assert.deepEqual(read, ["frame-0", "frame-0", "frame-1"]);
  assert.equal(images.urls.size, 2); assert.equal(registry.diagnostics().imageSlots, 2);
  runtime.dispose(); assert.equal(images.urls.size, 0); assert.equal(images.counts().live, 0);
});

test("full compatibility validation is sequential, closes decodes, and fails altered bytes/cancellation", async () => {
  const { pkg, client, read } = setup(), images = imageHost(), controller = new AbortController();
  await validateLocalPetSkinFrames(pkg, client, controller.signal, () => {}, images.host);
  assert.deepEqual(read, ["frame-0", "frame-1"]);
  assert.deepEqual(images.counts(), { decodes: 2, closes: 2, live: 0, maximum: 1 });
  assert.equal(images.urls.size, 0);
  await assert.rejects(validateLocalPetSkinFrames(pkg, { ...client, async readFrame() { return png(9); } }, controller.signal, () => {}, images.host), /完整性/);
  const waiting = deferred<ArrayBuffer>(), cancelled = new AbortController();
  const pending = validateLocalPetSkinFrames(pkg, { ...client, readFrame: () => waiting.promise }, cancelled.signal, () => {}, images.host);
  cancelled.abort(); waiting.resolve(png()); await assert.rejects(pending, { name: "AbortError" });
  assert.equal(images.counts().decodes, 2);
});

test("import/preview never select; validation and explicit Use gate selection; failed selection preserves old", async () => {
  const env = setup(), validation = deferred<void>();
  const owner = new LocalPetSkinController(env.client, async () => validation.promise);
  await owner.refresh();
  const pending = owner.importSkin(); await tick();
  assert.equal(owner.snapshot().phase, "validating");
  assert.deepEqual(env.selected, []); await owner.useSkin(); assert.deepEqual(env.selected, []);
  validation.resolve(); await pending;
  assert.equal(owner.snapshot().previewReady, true); assert.equal(await owner.useSkin(), true);
  assert.deepEqual(env.selected, [env.pkg.skinId]);
  env.client.select = async () => { throw Error("disk write failed"); };
  assert.equal(await owner.useBuiltin(), false);
  assert.equal(owner.snapshot().selected?.skinId, env.pkg.skinId); assert.match(owner.snapshot().error!, /disk write/);
  owner.dispose();
});

test("cancelled/bad imports retain existing choice and preview; selected event alone signals retry", async () => {
  const env = setup(), owner = new LocalPetSkinController(env.client, async () => {});
  await owner.start(); await tick(); await owner.previewSkin(env.pkg.skinId); await owner.useSkin();
  const previous = owner.snapshot();
  env.client.chooseImport = async () => null; await owner.importSkin();
  assert.equal(owner.snapshot().preview?.skinId, previous.preview?.skinId);
  env.client.chooseImport = async () => { throw Error("bad package"); }; await owner.importSkin();
  assert.equal(owner.snapshot().selected?.skinId, env.pkg.skinId);
  const revision = owner.snapshot().selectionRevision;
  env.notify("imported"); assert.equal(owner.snapshot().selectionRevision, revision);
  env.notify("selected"); assert.equal(owner.snapshot().selectionRevision, revision + 1);
  owner.dispose();
});

test("late validation after disposal cannot enable Use and subscriptions are removed", async () => {
  const env = setup(), wait = deferred<void>();
  const owner = new LocalPetSkinController(env.client, async () => wait.promise);
  await owner.start(); await tick(); const pending = owner.previewSkin(env.pkg.skinId); await tick();
  owner.dispose(); wait.resolve(); await pending;
  assert.equal(owner.snapshot().previewReady, false); assert.deepEqual(env.selected, []);
});

test("damaged package diagnostics permit built-in recovery/removal; a selected package cannot be removed", async () => {
  const env = setup(), corruptId = "b".repeat(64), removed: string[] = [];
  const initial = parseLocalPetSkinState({ selectedSkinId: env.pkg.skinId, skins: [env.pkg],
    problems: [{ skinId: corruptId, message: "皮肤清单损坏" }, { skinId: null, message: "选择记录损坏" }] });
  env.client.getState = async () => initial;
  env.client.remove = async (id) => { removed.push(id); return { selectedSkinId: env.pkg.skinId, skins: [env.pkg], problems: [] }; };
  const owner = new LocalPetSkinController(env.client, async () => {});
  await owner.refresh();
  assert.equal(await owner.removeSkin(env.pkg.skinId), false); assert.deepEqual(removed, []);
  assert.equal(await owner.removeSkin(corruptId), true); assert.deepEqual(removed, [corruptId]);
  assert.equal(await owner.removeSkin("c".repeat(64)), false);
  assert.equal(await owner.useBuiltin(), true); assert.equal(owner.snapshot().selected, null);
  assert.deepEqual(parseLocalPetSkinState({ selectedSkinId: null, skins: [] }).problems, []);
  owner.dispose();
});

test("failed frame validation cannot select or replace the existing selected skin", async () => {
  const env = setup(); await env.client.select(env.pkg.skinId); env.selected.length = 0;
  const owner = new LocalPetSkinController(env.client, async () => { throw Error("corrupt PNG"); });
  await owner.refresh(); await owner.importSkin();
  assert.equal(owner.snapshot().previewReady, false); assert.equal(await owner.useSkin(), false);
  assert.equal(owner.snapshot().selected?.skinId, env.pkg.skinId); assert.deepEqual(env.selected, []);
  owner.dispose();
});

test("frame gate commits only latest live ACK; failed ACK never displays", async () => {
  const first = deferred<void>(), second = deferred<void>(), commits: string[] = [];
  let live = true;
  const gate = createFrameDisplayGate((key) => key === "a" ? first.promise : second.promise, () => live);
  const a = gate("a", () => commits.push("a")), b = gate("b", () => commits.push("b"));
  second.resolve(); assert.equal(await b, true); first.resolve(); assert.equal(await a, false);
  assert.deepEqual(commits, ["b"]);
  live = false; assert.equal(await gate("b", () => commits.push("dead")), false);
  assert.equal(await createFrameDisplayGate(async () => { throw Error("no ACK"); }, () => true)("x", () => commits.push("bad")), false);
});

test("retiring custom URLs waits for the native clear ACK", async () => {
  const env = setup(), images = imageHost(), ack = deferred<void>();
  const runtime = createLocalPetSkinRuntime(env.pkg, env.client, () => {}, images.host);
  await runtime.getRegistry().load("idle", true);
  runtime.setBeforeRelease(() => ack.promise); runtime.dispose();
  assert.equal(images.urls.size, 1); ack.resolve(); await tick(); assert.equal(images.urls.size, 0);
});
