import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import type { AnimationClipDefinition } from "../features/pet/builtinAnimationSkin";
import { roamingAnimation, roamingFrameReady, roamingPlayback } from "../features/pet/roamingAnimation";
import type { DesktopCompanionState } from "./desktopCompanion";

const manifest = JSON.parse(readFileSync(new URL("../../assets/pets/nekomi/manifest-animation-v12.json", import.meta.url), "utf8"));
const clips = manifest.clips as AnimationClipDefinition[];
const moving: DesktopCompanionState = {
  runtimeEpoch: "native-a", revision: "2", motionGeneration: "7", phase: "moving", pauseReason: null,
  inputIdleMs: 125_000, motion: { direction: "right", remainingMs: 2400 }, activeApp: null,
  capabilities: { systemIdle: true, roaming: true, workApps: true, launchers: true, fileDrop: false, windowEdges: false },
};
const allowed = { available: true, eligible: true, enabled: true, scope: "source-a" };

test("受限移动复用八帧单向循环，原地预览清单保持原样", () => {
  for (const direction of ["left", "right"] as const) {
    const original = structuredClone(clips.find((clip) => clip.id === `walk-${direction}`));
    const playback = roamingPlayback(clips, direction)!;
    assert.equal(playback.loop, true);
    assert.equal(playback.sequence.length, 8);
    assert.equal(playback.loopRange, undefined);
    assert.equal(playback.fallback, playback.sequence[0].frame);
    assert.ok(playback.sequence.every((step) => step.segment === "body"));
    assert.equal(playback.sequence.reduce((sum, step) => sum + step.durationMs, 0), 1280);
    assert.deepEqual(clips.find((clip) => clip.id === `walk-${direction}`), original);
  }
  assert.equal(roamingPlayback([], "left"), null);
  const broken = structuredClone(clips);
  broken.find((clip) => clip.id === "walk-left")!.sequence.find((step) => step.segment === "body")!.frame = "wrong-direction";
  assert.equal(roamingPlayback(broken, "left"), null);
});

test("关闭、任务活动、未知域、本地皮肤与原生暂停均不能请求移动动画", () => {
  assert.equal(roamingAnimation(moving, allowed)?.clip, "walk-right");
  for (const options of [{ ...allowed, available: false }, { ...allowed, eligible: false },
    { ...allowed, enabled: false }, { ...allowed, scope: null }]) assert.equal(roamingAnimation(moving, options), null);
  for (const state of [null, undefined, { ...moving, phase: "idle" as const, motion: null },
    { ...moving, pauseReason: "workActive" }, { ...moving, inputIdleMs: null }, { ...moving, inputIdleMs: 119_999 },
    { ...moving, motion: { direction: "right" as const, remainingMs: 0 } },
    { ...moving, capabilities: { ...moving.capabilities, roaming: false } }])
    assert.equal(roamingAnimation(state, allowed), null);
});

test("方向、宿主、来源和段代次独立绑定；普通状态修订不重新播放", () => {
  const first = roamingAnimation(moving, allowed)!;
  assert.equal(roamingAnimation({ ...moving, revision: "3", motion: { direction: "right", remainingMs: 10 } }, allowed)?.key, first.key);
  for (const state of [{ ...moving, runtimeEpoch: "native-b" }, { ...moving, motionGeneration: "8" },
    { ...moving, motion: { direction: "left" as const, remainingMs: 2400 } }])
    assert.notEqual(roamingAnimation(state, allowed)?.key, first.key);
  assert.notEqual(roamingAnimation(moving, { ...allowed, scope: "source-b" })?.key, first.key);
});

test("只有当前方向已绘制的真实步态帧能确认；迟到、失败和本地帧不能授予许可", () => {
  const request = roamingAnimation(moving, allowed)!, playback = roamingPlayback(clips, "right")!;
  const sprite = { current: true, clip: request.clip, scope: request.key, runtimeKey: "builtin", error: false, frame: playback.sequence[0].frame };
  assert.equal(roamingFrameReady(request, sprite, request.key, playback), true);
  assert.equal(roamingFrameReady(request, sprite, null, playback), false);
  for (const patch of [{ current: false }, { error: true }, { runtimeKey: "local:any" },
    { clip: "walk-left" }, { scope: "old-domain" }, { frame: "walk-v10-complete/model-reference/master-front-v2" }])
    assert.equal(roamingFrameReady(request, { ...sprite, ...patch }, request.key, playback), false);
  const next = roamingAnimation({ ...moving, motionGeneration: "8" }, allowed)!;
  assert.equal(roamingFrameReady(next, sprite, request.key, playback), false);
  assert.equal(roamingFrameReady(null, sprite, request.key, playback), false);
});
