import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { AnimationClipRegistry, type AnimationClipDefinition, type AnimationClipHost,
  type AnimationFrameDefinition, loadAnimationWindow, planAnimationWindow } from "../features/pet/animationClipRegistry";
import { KeyframePlayer, playbackClip } from "../features/pet/keyframePlayer";
import { bindPetWindowLifetime } from "../features/pet/petWindowLifetime";

const RGBA = 1024 * 1536 * 4;

// These small structural PNGs exercise the registry trust boundary with an
// injected decoder. They are not browser pixel-decoding or Native evidence.
function structuralPng(marker: number) {
  const bytes = new Uint8Array(58);
  bytes.set([137, 80, 78, 71, 13, 10, 26, 10]);
  const view = new DataView(bytes.buffer);
  view.setUint32(8, 13);
  bytes.set([73, 72, 68, 82], 12);
  view.setUint32(16, 1024);
  view.setUint32(20, 1536);
  bytes.set([8, 6, 0, 0, 0], 24);
  view.setUint32(33, 1);
  bytes.set([73, 68, 65, 84, marker], 37);
  bytes.set([73, 69, 78, 68], 50);
  return bytes;
}

function fixture() {
  const frames: AnimationFrameDefinition[] = [], sources: Record<string, string> = {};
  const bytes = new Map<string, Uint8Array>();
  for (let index = 0; index < 38; index++) {
    const id = `frame/${index}`, png = structuralPng(index);
    sources[id] = `https://pet.test/assets/pets/test/${index}.png`;
    bytes.set(sources[id], png);
    frames.push({ id, path: `assets/pets/test/${index}.png`, width: 1024, height: 1536,
      bytes: png.byteLength, sha256: createHash("sha256").update(png).digest("hex"),
      bounds: [12, 20, 1000, 1490] });
  }
  const clips: AnimationClipDefinition[] = [
    { id: "idle", fallback: "frame/0", sequence: [0, 1, 0, 2, 3, 4, 0].map((index) =>
      ({ frame: `frame/${index}`, durationMs: 100 })) },
    { id: "active", fallback: "frame/5", sequence: [5, 6, 7, 5].map((index) =>
      ({ frame: `frame/${index}`, durationMs: 100 })) },
    { id: "complete", fallback: "frame/8", sequence: [8, 9, 10, 8].map((index) =>
      ({ frame: `frame/${index}`, durationMs: 100 })) },
    { id: "sleep", fallback: "frame/11", sequence: [{ frame: "frame/11", durationMs: 100 }] },
  ];
  return { frames, sources, bytes, clips };
}
type Fixture = ReturnType<typeof fixture>;

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function tracker(data: Fixture, limit = 5) {
  const urls = new Set<string>(), signals: AbortSignal[] = [], fetched: string[] = [];
  let decoded = 0, closed = 0, created = 0, liveBitmaps = 0, maxSlots = 0, maxDecoders = 0;
  function sample() {
    maxSlots = Math.max(maxSlots, urls.size + liveBitmaps);
    assert.ok(urls.size + liveBitmaps <= limit, "URLs and pending decoders fit the declared logical slots");
  }
  const host: AnimationClipHost = {
    async fetch(source, signal) {
      fetched.push(source);
      signals.push(signal);
      const bytes = data.bytes.get(source);
      assert.ok(bytes, "only declared frame URLs may be requested");
      return new Response(new Uint8Array(bytes).buffer);
    },
    async decode() {
      decoded++;
      liveBitmaps++;
      maxDecoders = Math.max(maxDecoders, liveBitmaps);
      sample();
      let released = false;
      return { width: 1024, height: 1536, corners: [0, 0, 0, 0], close() {
        assert.equal(released, false, "each decoder closes exactly once");
        released = true;
        liveBitmaps--;
        closed++;
        sample();
      } };
    },
    createUrl() {
      const url = `blob:animation-${++created}`;
      urls.add(url);
      sample();
      return url;
    },
    revokeUrl(url) {
      assert.ok(urls.delete(url), "only a live owned URL is revoked, once");
      sample();
    },
  };
  return { host, urls, signals, fetched,
    counts: () => ({ decoded, closed, created, liveBitmaps, maxSlots, maxDecoders }) };
}

test("animation reads only the active clip's distinct frames, deduplicates and stays within five slots", async () => {
  const data = fixture(), count = tracker(data);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  assert.equal(count.fetched.length, 0, "38 catalog entries do not preload PNGs");
  const pending = registry.load("idle");
  assert.equal(registry.load("idle"), pending);
  const idle = await pending;
  assert.deepEqual(Object.keys(idle!.frames), [0, 1, 2, 3, 4].map((i) => `frame/${i}`));
  assert.equal(Object.isFrozen(idle), true);
  assert.equal(Object.isFrozen(idle!.frames), true);
  assert.equal(Object.isFrozen(idle!.frames["frame/0"]), true);
  assert.equal(count.fetched.length, 5);
  assert.equal(registry.load("idle"), pending, "a settled active clip also shares its result");
  assert.equal(registry.diagnostics().pending, false);
  assert.equal(registry.diagnostics().decodedBytes, 5 * RGBA);
  assert.equal(count.counts().maxSlots, 5);
  const active = registry.load("active");
  assert.equal(count.urls.size, 0, "transition synchronously releases the previous clip");
  assert.equal((await active)?.clip, "active");
  assert.equal(count.fetched.length, 8);
  assert.equal(count.counts().maxDecoders, 1);
  assert.equal(count.counts().closed, 8);
  assert.equal(registry.diagnostics().decodedBytes, 3 * RGBA);
  registry.dispose();
  assert.equal(count.urls.size, 0);
});

test("a twelve-frame walk stays serial and releases all slots on static replacement", async () => {
  const data = fixture(), count = tracker(data, 12);
  data.clips.push({ id: "walk", fallback: "frame/0", sequence: Array.from({ length: 26 }, (_, i) =>
    ({ frame: `frame/${i % 12}`, durationMs: 160 })) });
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  assert.equal(Object.keys((await registry.load("walk"))!.frames).length, 12);
  assert.equal(count.fetched.length, 12);
  assert.equal(count.counts().maxDecoders, 1);
  assert.equal(count.counts().maxSlots, 12);
  assert.equal(registry.diagnostics().decodedBytes, 72 * 1024 * 1024);
  const reduced = registry.load("walk", true);
  assert.equal(count.urls.size, 0);
  assert.deepEqual(Object.keys((await reduced)!.frames), ["frame/0"]);
  assert.equal(registry.diagnostics().objectUrls, 1);
  registry.cancel();
  assert.equal(count.urls.size, 0);
  registry.dispose();
});

test("static-only mode reads exactly fallback, cancels animation and can return to a fresh clip", async () => {
  const data = fixture(), count = tracker(data);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  const staticClip = await registry.load("idle", true);
  assert.deepEqual(Object.keys(staticClip!.frames), ["frame/0"]);
  assert.deepEqual(count.fetched, [data.sources["frame/0"]]);
  assert.equal(registry.diagnostics().staticOnly, true);
  const animated = await registry.load("idle");
  assert.equal(Object.keys(animated!.frames).length, 5);
  assert.equal(count.fetched.length, 6);
  await registry.load("idle", true);
  assert.equal(count.fetched.length, 7);
  assert.equal(count.urls.size, 1);
  registry.dispose();
});

test("latest selection aborts stale fetches and queued clips cannot revive or fetch later frames", async () => {
  const data = fixture(), count = tracker(data), entered = deferred(), release = deferred();
  const original = count.host.fetch;
  let firstSignal: AbortSignal | undefined, cancelledBodies = 0;
  count.host.fetch = async (source, signal) => {
    if (source === data.sources["frame/0"]) {
      firstSignal = signal;
      entered.resolve();
      await release.promise; // Deliberately ignore abort; generation must still fence this response.
      return new Response(new ReadableStream({ cancel() { cancelledBodies++; } }));
    }
    return original(source, signal);
  };
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  const stale = registry.load("idle");
  await entered.promise;
  const skipped = registry.load("active"), latest = registry.load("complete");
  assert.equal(firstSignal?.aborted, true);
  release.resolve();
  assert.equal(await stale, null);
  assert.equal(await skipped, null);
  assert.equal((await latest)?.clip, "complete");
  assert.equal(cancelledBodies, 1);
  assert.deepEqual(count.fetched, [8, 9, 10].map((i) => data.sources[`frame/${i}`]));
  assert.equal(count.counts().created, 3);
  registry.dispose();
});

test("changing to static-only cancels a streaming animation read and never fetches its remaining frames", async () => {
  const data = fixture(), count = tracker(data), entered = deferred();
  const original = count.host.fetch;
  let cancelled = 0, streamingSignal: AbortSignal | undefined;
  count.host.fetch = async (source, signal) => {
    if (source === data.sources["frame/1"]) {
      streamingSignal = signal;
      return new Response(new ReadableStream({ start(controller) {
        controller.enqueue(data.bytes.get(source)!.subarray(0, 10));
        entered.resolve();
      }, cancel() { cancelled++; } }));
    }
    return original(source, signal);
  };
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  const old = registry.load("idle");
  await entered.promise;
  const latest = registry.load("idle", true);
  assert.equal(streamingSignal?.aborted, true);
  assert.equal(count.urls.size, 0);
  assert.equal(await old, null);
  assert.deepEqual(Object.keys((await latest)!.frames), ["frame/0"]);
  assert.equal(cancelled, 1);
  assert.deepEqual(count.fetched, [data.sources["frame/0"], data.sources["frame/0"]]);
  registry.dispose();
});

test("a late decoder closes once, remains serial and cannot publish or evict the replacement clip", async () => {
  const data = fixture(), count = tracker(data), entered = deferred(), release = deferred();
  const original = count.host.decode;
  let calls = 0;
  count.host.decode = async (blob) => {
    const bitmap = await original(blob);
    if (++calls === 2) { entered.resolve(); await release.promise; }
    return bitmap;
  };
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  const stale = registry.load("idle");
  await entered.promise;
  assert.equal(count.urls.size, 1);
  const latest = registry.load("active");
  assert.equal(count.urls.size, 0);
  assert.equal(registry.diagnostics().pendingDecodes, 1);
  release.resolve();
  assert.equal(await stale, null);
  const active = await latest;
  assert.equal(active?.clip, "active");
  assert.equal(count.counts().decoded, 5);
  assert.equal(count.counts().closed, 5);
  assert.equal(count.counts().created, 4, "stale pending bitmap never creates a URL");
  assert.equal(count.counts().maxDecoders, 1);
  assert.equal(count.urls.size, 3);
  registry.dispose();
});

test("disposal releases partial clips and closes a late bitmap without resurrection", async () => {
  const data = fixture(), count = tracker(data), entered = deferred(), release = deferred(), closed = deferred();
  const original = count.host.decode;
  let calls = 0;
  count.host.decode = async (blob) => {
    const bitmap = await original(blob);
    if (++calls === 2) {
      entered.resolve();
      await release.promise;
      return { ...bitmap, close() { bitmap.close(); closed.resolve(); } };
    }
    return bitmap;
  };
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  const pending = registry.load("idle");
  await entered.promise;
  registry.dispose();
  registry.dispose();
  assert.equal(count.urls.size, 0);
  assert.equal(registry.diagnostics().pendingDecodes, 1);
  assert.equal(await pending, null, "dispose settles callers before a non-cancellable decode returns");
  release.resolve();
  await closed.promise;
  assert.equal(count.counts().closed, 2);
  assert.equal(count.counts().created, 1);
  assert.equal(registry.diagnostics().decodedBytes, 0);
  assert.equal(await registry.load("complete"), null);
});

test("a blocked read retains only one latest candidate and replaced callers settle before it returns", async () => {
  const data = fixture(), count = tracker(data), entered = deferred(), release = deferred();
  const original = count.host.fetch;
  count.host.fetch = async (source, signal) => {
    if (source === data.sources["frame/0"]) { entered.resolve(); await release.promise; }
    return original(source, signal);
  };
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  const first = registry.load("idle");
  await entered.promise;
  const pending = [];
  for (let index = 0; index < 1000; index++) {
    pending.push(registry.load(index % 2 ? "complete" : "active"));
    assert.equal(registry.diagnostics().queuedRequests, 1);
  }
  assert.equal(await first, null);
  assert.deepEqual(await Promise.all(pending.slice(0, -1)), Array(999).fill(null));
  assert.equal(count.fetched.length, 0);
  assert.equal(count.counts().decoded, 0);
  release.resolve();
  assert.equal((await pending.at(-1))?.clip, "complete");
  assert.deepEqual(count.fetched, [0, 8, 9, 10].map((i) => data.sources[`frame/${i}`]));
  assert.equal(count.counts().decoded, 3);
  assert.equal(registry.diagnostics().queuedRequests, 0);
  registry.dispose();
});

test("cancel clears executing and candidate work, closes late decode, and keeps the owner reusable", async () => {
  const data = fixture(), count = tracker(data), entered = deferred(), release = deferred(), closed = deferred();
  const original = count.host.decode;
  let calls = 0;
  count.host.decode = async (blob) => {
    const bitmap = await original(blob);
    if (++calls === 2) {
      entered.resolve();
      await release.promise;
      return { ...bitmap, close() { bitmap.close(); closed.resolve(); } };
    }
    return bitmap;
  };
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  const executing = registry.load("idle");
  await entered.promise;
  const candidate = registry.load("active");
  registry.cancel();
  assert.equal(await executing, null);
  assert.equal(await candidate, null);
  assert.equal(count.urls.size, 0);
  assert.equal(registry.diagnostics().clip, null);
  assert.equal(registry.diagnostics().queuedRequests, 0);
  assert.equal(registry.diagnostics().disposed, false);
  const reused = registry.load("sleep", true);
  release.resolve();
  await closed.promise;
  assert.equal((await reused)?.clip, "sleep");
  assert.deepEqual(count.fetched, [0, 1, 11].map((i) => data.sources[`frame/${i}`]));
  assert.equal(count.counts().closed, 3);
  registry.cancel();
  assert.equal(count.urls.size, 0);
  assert.equal((await registry.load("idle", true))?.clip, "idle");
  registry.dispose();
});

test("bad streams, SHA, PNG headers and redirects fail before decode and leave no previous celebration", async () => {
  const data = fixture(), count = tracker(data);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  await registry.load("complete");
  const original = count.host.fetch, bytes = data.bytes.get(data.sources["frame/0"])!;
  for (const fault of ["hash", "header", "length", "short", "dimensions", "redirect", "http"] as const) {
    count.host.fetch = async () => {
      const damaged = new Uint8Array(bytes.length + (fault === "length" ? 1 : fault === "short" ? -1 : 0));
      damaged.set(bytes.subarray(0, damaged.length));
      if (fault === "hash") damaged[41] ^= 1;
      if (fault === "header") damaged[0] = 0;
      if (fault === "dimensions") new DataView(damaged.buffer).setUint32(16, 512);
      const response = new Response(damaged.buffer, { status: fault === "http" ? 404 : 200 });
      if (fault === "redirect") Object.defineProperty(response, "redirected", { value: true });
      return response;
    };
    await assert.rejects(registry.load("idle"));
    assert.equal(count.urls.size, 0);
    assert.equal(count.counts().decoded, 3);
    assert.equal(registry.diagnostics().pending, false);
  }
  count.host.fetch = original;
  assert.equal((await registry.load("idle", true))?.clip, "idle", "failure permits explicit retry");
  registry.dispose();
});

test("decoded originals permit only integer alpha 0..1 corners and always close rejected bitmaps", async () => {
  const data = fixture(), count = tracker(data);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  const original = count.host.decode;
  for (const fault of ["throw", "width", "height", "opaque", "two", "fractional", "negative", "missing"] as const) {
    count.host.decode = async (blob) => {
      if (fault === "throw") throw Error("injected decoder fault");
      const bitmap = await original(blob);
      return { ...bitmap, width: fault === "width" ? 512 : bitmap.width,
        height: fault === "height" ? 1024 : bitmap.height,
        corners: fault === "opaque" ? [0, 255, 0, 0] : fault === "two" ? [2, 0, 0, 0] :
          fault === "fractional" ? [0.5, 0, 0, 0] : fault === "negative" ? [-1, 0, 0, 0] :
          fault === "missing" ? [0, 0, 0] : bitmap.corners };
    };
    await assert.rejects(registry.load("idle", true));
    assert.equal(count.urls.size, 0);
    assert.equal(count.counts().liveBitmaps, 0);
    assert.equal(registry.diagnostics().pendingDecodes, 0);
  }
  count.host.decode = async (blob) => ({ ...await original(blob), corners: [1, 0, 0, 0] });
  assert.equal((await registry.load("idle", true))?.clip, "idle");
  assert.equal(count.counts().closed, 8);
  registry.dispose();
});

test("partial new clip failure releases its own URLs and a retry reads the full clip afresh", async () => {
  const data = fixture(), count = tracker(data), original = count.host.fetch;
  count.host.fetch = async (source, signal) => source === data.sources["frame/2"] ?
    new Response(new Uint8Array(1).buffer) : original(source, signal);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  await assert.rejects(registry.load("idle"));
  assert.equal(count.counts().created, 2);
  assert.equal(count.urls.size, 0);
  assert.equal(registry.diagnostics().clip, null);
  count.host.fetch = original;
  assert.equal(Object.keys((await registry.load("idle"))!.frames).length, 5);
  assert.equal(count.counts().created, 7);
  registry.dispose();
});

test("an unknown clip also cancels pending work and releases an existing celebration", async () => {
  const data = fixture(), count = tracker(data);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  await registry.load("complete");
  await assert.rejects(registry.load("unknown"), /未知/);
  assert.equal(count.urls.size, 0);
  assert.equal(registry.diagnostics().clip, null);
  assert.equal((await registry.load("idle", true))?.clip, "idle");
  registry.dispose();
});

test("catalog rejects invalid bounds, unsafe paths, extra frames, excessive clips and both encoded budgets", () => {
  const data = fixture();
  const mutations: ((frames: AnimationFrameDefinition[], clips: AnimationClipDefinition[]) => void)[] = [
    (frames) => { frames[0].path = "../idle.png"; },
    (frames) => { frames[0].path = "assets/pets/../idle.png"; },
    (frames) => { frames[0].bytes = 4 * 1024 * 1024 + 1; },
    (frames) => { frames[0].width = 2048; },
    (frames) => { frames[0].bounds = [0, 0, 1025, 1536]; },
    (frames) => { frames[0].bounds = [0, -1, 1024, 1536]; },
    (frames) => { frames[0].bounds = [20, 0, 20, 1536]; },
    (frames) => { frames[0].bounds = [0.5, 0, 1024, 1536]; },
    (frames) => { frames[1].id = frames[0].id; },
    (frames) => { frames[1].path = frames[0].path; },
    (frames) => { frames.push({ ...frames[0], id: "extra", path: "assets/pets/extra.png" }); },
    (frames) => { for (const frame of frames) frame.bytes = 4 * 1024 * 1024; },
    (_, clips) => { clips[0].fallback = "missing"; },
    (_, clips) => { clips[0].sequence = [{ frame: "missing", durationMs: 1 }]; },
    (_, clips) => { clips[0].sequence = [{ frame: "frame/0", durationMs: 0 }]; },
    (_, clips) => { clips[0].sequence = [{ frame: "frame/0", durationMs: Infinity }]; },
    (_, clips) => { clips[0].sequence = []; },
    (_, clips) => { clips[1].id = clips[0].id; },
    (_, clips) => { clips.push(...Array.from({ length: 38 }, (_, i) => ({ ...clips[0], id: `extra${i}` }))); },
  ];
  for (const mutate of mutations) {
    const frames = structuredClone(data.frames), clips = structuredClone(data.clips);
    mutate(frames, clips);
    assert.throws(() => new AnimationClipRegistry(frames, data.sources, clips));
  }
  assert.throws(() => new AnimationClipRegistry(data.frames, {}, data.clips));
});

test("catalog copies author data so mutations cannot enlarge or replace the active request", async () => {
  const data = fixture(), count = tracker(data);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  data.frames[0].sha256 = "f".repeat(64);
  data.frames[0].bounds = [-1, -1, -1, -1];
  data.sources["frame/0"] = "https://other.test/modified.png";
  data.clips[0].fallback = "frame/37";
  data.clips[0].sequence = [{ frame: "frame/37", durationMs: 100 }];
  assert.deepEqual(Object.keys((await registry.load("idle", true))!.frames), ["frame/0"]);
  registry.dispose();
});

test("default reader rejects cross-origin, credentials and data URLs before the network", async () => {
  const data = fixture(), priorWindow = Object.getOwnPropertyDescriptor(globalThis, "window"),
    priorFetch = globalThis.fetch;
  let networkCalls = 0;
  Object.defineProperty(globalThis, "window", { configurable: true,
    value: { location: { href: "https://pet.test/index.html" } } });
  globalThis.fetch = async () => { networkCalls++; throw Error("network disabled"); };
  try {
    for (const source of ["https://other.test/idle.png", "https://name:password@pet.test/idle.png",
      "http://pet.test/idle.png", "data:image/png;base64,AA==", "blob:https://pet.test/value"]) {
      const registry = new AnimationClipRegistry(data.frames,
        { ...data.sources, "frame/0": source }, data.clips);
      try { await assert.rejects(registry.load("idle", true), /同源/); }
      finally { registry.dispose(); }
    }
    assert.equal(networkCalls, 0);
  } finally {
    globalThis.fetch = priorFetch;
    if (priorWindow) Object.defineProperty(globalThis, "window", priorWindow);
    else Reflect.deleteProperty(globalThis, "window");
  }
});

test("window lifetime keeps BFCache, while terminal pagehide releases all current frames once", async () => {
  const data = fixture(), count = tracker(data);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  await registry.load("idle");
  const target = new EventTarget();
  let disposals = 0;
  const teardown = bindPetWindowLifetime({ dispose() { disposals++; registry.dispose(); } }, target);
  const cached = new Event("pagehide");
  Object.defineProperty(cached, "persisted", { value: true });
  target.dispatchEvent(cached);
  assert.equal(count.urls.size, 5);
  target.dispatchEvent(new Event("pagehide"));
  assert.equal(count.urls.size, 0);
  teardown();
  target.dispatchEvent(new Event("pagehide"));
  assert.equal(disposals, 1);
  assert.equal(await registry.load("idle"), null);
});

function turnFixture() {
  const data = fixture();
  // Thirty-two reviewed steps, including a repeated reverse tail, need twenty-
  // three distinct images. A window must not trim either the steps or the tail.
  const indices = [...Array.from({ length: 23 }, (_, i) => i),
    ...Array.from({ length: 9 }, (_, i) => 22 - i)];
  const turn = { id: "turn", fallback: "frame/0", loop: false,
    sequence: indices.map((i) => ({ frame: `frame/${i}`, durationMs: 100 })) };
  data.clips.push(turn);
  return { data, turn };
}

test("long clips require explicit windows and static-only still reads just fallback", async () => {
  const { data, turn } = turnFixture(), count = tracker(data, 12);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  await assert.rejects(registry.load("turn"), /有界窗口/);
  assert.equal(count.fetched.length, 0);
  assert.deepEqual(Object.keys((await registry.load("turn", true))!.frames), [turn.fallback]);
  assert.equal(count.counts().maxSlots, 1);
  const first = planAnimationWindow(turn, 0);
  assert.deepEqual(first, Array.from({ length: 12 }, (_, i) => `frame/${i}`));
  assert.equal(Object.keys((await registry.loadWindow("turn", first))!.frames).length, 12);
  assert.equal(count.counts().maxSlots, 12);
  for (const ids of [[], [...first, "frame/12"], ["frame/1", "frame/1"], ["frame/30"], ["undeclared"]]) {
    await assert.rejects(registry.loadWindow("turn", ids), /窗口/);
    assert.equal(count.urls.size, 0, "invalid admission releases the prior window");
  }
  assert.throws(() => planAnimationWindow(turn, -1), /起点/);
  assert.throws(() => planAnimationWindow(turn, 32), /起点/);
  registry.dispose();
});

test("a 23-image turn preserves all 32 timed steps across ACK and serial IO within twelve slots", async () => {
  const { data, turn } = turnFixture(), count = tracker(data, 12);
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  let asset = (await registry.loadWindow("turn", planAnimationWindow(turn, 0)))!;
  const clock = new KeyframePlayer(), seen: string[] = [];
  clock.start(playbackClip(turn), 0);
  let now = 0, boundaries = 0;
  for (const [index, step] of turn.sequence.entries()) {
    let sample = clock.readPosition(now);
    assert.equal(sample.stepIndex, index);
    assert.equal(sample.nextInMs, step.durationMs);
    if (!asset.frames[sample.frame]) {
      const paused = clock.pause(now), ack = deferred<boolean>();
      const priorUrls = [...count.urls], priorReads = count.fetched.length;
      const next = loadAnimationWindow(registry, "turn", planAnimationWindow(turn, paused.stepIndex),
        () => ack.promise, () => true);
      now += 50_000; // Long renderer ACK/IO must not consume any turn step.
      assert.deepEqual(clock.readPosition(now), paused);
      assert.deepEqual([...count.urls], priorUrls, "old images remain owned until native clear ACK");
      assert.equal(count.fetched.length, priorReads, "no preload while clear is pending");
      ack.resolve(true);
      asset = (await next)!;
      sample = clock.resume(now);
      assert.deepEqual(sample, paused);
      assert.ok(priorUrls.every((url) => !count.urls.has(url)), "ACK precedes old URL release");
      boundaries++;
    }
    assert.ok(asset.frames[sample.frame]);
    seen.push(sample.frame);
    now += step.durationMs;
  }
  assert.deepEqual(seen, turn.sequence.map((step) => step.frame));
  assert.equal(boundaries, 1);
  assert.deepEqual(clock.readPosition(now), { frame: turn.sequence.at(-1)!.frame,
    nextInMs: null, stepIndex: null });
  assert.equal(count.fetched.length, 23);
  assert.equal(count.counts().maxSlots, 12);
  assert.equal(count.counts().maxDecoders, 1);
  assert.equal(count.counts().closed, 23);
  registry.dispose();
  assert.equal(count.urls.size, 0);
});

test("a late old-window ACK cannot evict the replacement's images or fetch stale frames", async () => {
  const { data, turn } = turnFixture(), count = tracker(data, 12), ack = deferred<boolean>();
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  await registry.loadWindow("turn", planAnimationWindow(turn, 0));
  let live = true;
  const old = loadAnimationWindow(registry, "turn", planAnimationWindow(turn, 12),
    () => ack.promise, () => live);
  live = false;
  const replacement = await registry.load("active");
  const urls = [...count.urls], reads = [...count.fetched];
  ack.resolve(true);
  assert.equal(await old, null);
  assert.equal(replacement?.clip, "active");
  assert.deepEqual([...count.urls], urls);
  assert.deepEqual(count.fetched, reads);
  assert.equal(registry.diagnostics().clip, "active");
  const denied = await loadAnimationWindow(registry, "turn", planAnimationWindow(turn, 12),
    async () => false, () => true);
  assert.equal(denied, null);
  assert.deepEqual([...count.urls], urls, "failed clear also retains the current window");
  registry.dispose();
});

test("blocked long-window decode retains only its latest replacement and never exceeds twelve slots", async () => {
  const { data } = turnFixture(), count = tracker(data, 12), entered = deferred(), release = deferred();
  const original = count.host.decode;
  let calls = 0;
  count.host.decode = async (blob) => {
    const bitmap = await original(blob);
    if (++calls === 1) { entered.resolve(); await release.promise; }
    return bitmap;
  };
  const registry = new AnimationClipRegistry(data.frames, data.sources, data.clips, count.host);
  const first = registry.loadWindow("turn", ["frame/0", "frame/1"]);
  await entered.promise;
  const pending = [];
  for (let index = 0; index < 1000; index++) {
    pending.push(registry.loadWindow("turn", [`frame/${index % 23}`]));
    assert.equal(registry.diagnostics().queuedRequests, 1);
    assert.equal(registry.diagnostics().pendingDecodes, 1);
  }
  assert.equal(await first, null);
  assert.deepEqual(await Promise.all(pending.slice(0, -1)), Array(999).fill(null));
  assert.equal(count.fetched.length, 1);
  release.resolve();
  assert.deepEqual(Object.keys((await pending.at(-1))!.frames), [`frame/${999 % 23}`]);
  assert.equal(count.fetched.length, 2);
  assert.equal(count.counts().created, 1, "late cancelled decode never creates a URL");
  assert.equal(count.counts().closed, 2);
  assert.equal(count.counts().maxDecoders, 1);
  registry.dispose();
  assert.equal(count.urls.size, 0);
});
