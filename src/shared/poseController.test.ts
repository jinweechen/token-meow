import assert from "node:assert/strict";
import test from "node:test";
import { PET_REACTIONS } from "../features/pet/poseController";

test("三种轻互动共用冷却和事实准入，未知动作与迟到播放回调不能覆盖当前姿态", () => {
  const poses = new PoseController({ idle: [], tap: [7, 9, 8] }, () => 1800, true);
  assert.equal(poses.tap(true, 0, "unknown"), false);
  let now = 0;
  for (const reaction of PET_REACTIONS) {
    assert.equal(poses.tap(true, now, reaction.id), true);
    assert.deepEqual(poses.tick(true, now), { kind: "tap", frame: reaction.frame });
    const instance = poses.playbackKey()!;
    assert.equal(poses.tap(true, now + 100, "wink"), false);
    poses.interrupt();
    poses.startPlayback(instance, now + 200);
    assert.equal(poses.tick(true, now + 200, false), null);
    now += 3000;
  }
  assert.equal(poses.tap(false, now, "affection"), false);
  assert.equal(poses.tick(true, now, false), null);
});
import { allowsCompanionPose, PoseController, POSE_LIMITS } from "../features/pet/poseController";
import type { DesktopCompanionState } from "./desktopCompanion";

test("production pose controller rotates idle variants at low frequency without adding a scheduler", () => {
  const poses = new PoseController({ idle: [6, 7], tap: [8, 9] });
  assert.equal(poses.tick(true, 1000), null);
  assert.equal(poses.tick(true, 1000 + POSE_LIMITS.idleInterval - 1), null);
  assert.deepEqual(poses.tick(true, 19000), { frame: 6, kind: "idle" });
  assert.equal(poses.tick(true, 21400), null);
  assert.deepEqual(poses.tick(true, 37000), { frame: 7, kind: "idle" });
});
test("production tap pose is bounded, cooled down and immediately preempted by real state or pause", () => {
  const poses = new PoseController({ idle: [6], tap: [8, 9] });
  assert.equal(poses.tap(true, 1000), true);
  assert.deepEqual(poses.tick(true, 1001), { frame: 8, kind: "tap" });
  assert.equal(poses.tap(true, 2000), false);
  assert.equal(poses.tick(false, 2100), null);
  assert.equal(poses.tick(true, 100000), null);
  assert.equal(poses.tap(false, 100001), false);
  assert.equal(poses.tap(true, 100002), true);
  assert.deepEqual(poses.tick(true, 100003), { frame: 9, kind: "tap" });
  assert.equal(poses.tick(true, 101802), null);
  poses.destroy();
  assert.equal(poses.tap(true, 200000), false);
  assert.equal(poses.tick(true, 200001), null);
});

test("finite cute, sleep and walk poses use complete asset durations after loading", () => {
  for (const [frame, duration] of [[6, 3100], [4, 11450], [13, 5670]]) {
    const poses = new PoseController({ idle: [frame], tap: [frame] }, () => duration, true);
    assert.equal(poses.tap(true, 1000), true);
    const instance = poses.playbackKey()!;
    assert.deepEqual(poses.tick(true, 15_000), { frame, kind: "tap" });
    poses.startPlayback(instance, 15_000);
    poses.startPlayback(instance, 16_000); // Repeated frame samples cannot renew the deadline.
    assert.deepEqual(poses.tick(true, 15_000 + duration - 1), { frame, kind: "tap" });
    assert.equal(poses.tick(true, 15_000 + duration), null);
  }
});

test("native play generations trigger once and may finish after native returns to idle", () => {
  const poses = new PoseController({ idle: [13], tap: [6] }, () => 5670);
  assert.equal(poses.play(true, "epoch-a/7", 13, 1000), true);
  const instance = poses.playbackKey();
  assert.equal(poses.play(true, "epoch-a/7", 13, 3000), false);
  assert.deepEqual(poses.tick(true, 4000, false), { frame: 13, kind: "idle" });
  assert.equal(poses.playbackKey(), instance);
  assert.equal(poses.tick(true, 6670, false), null);
  assert.equal(poses.play(true, "epoch-a/7", 13, 7000), false);
  assert.equal(poses.play(true, "epoch-a/8", 13, 8000), true);
  poses.stopCompanion(); // Native input, pause, loss or disabled play interrupts immediately.
  assert.equal(poses.tick(true, 8001, false), null);
  assert.equal(poses.play(true, "epoch-a/8", 13, 9000), false);
  assert.equal(poses.play(false, "epoch-a/9", 13, 10_000), false);
  assert.equal(poses.play(true, "epoch-a/9", 13, 11_000), false);
  assert.equal(poses.tap(true, 12_000), true);
  assert.equal(poses.play(true, "epoch-a/10", 13, 12_001), false);
  poses.stopCompanion();
  assert.deepEqual(poses.tick(true, 12_002, false), { frame: 6, kind: "tap" });
  assert.equal(poses.tick(false, 12_003, false), null);
});

test("late asset callbacks cannot revive or extend a replaced pose", () => {
  const poses = new PoseController({ idle: [], tap: [6, 7] }, () => 3100, true);
  poses.tap(true, 0);
  const old = poses.playbackKey()!;
  poses.interrupt();
  poses.tap(true, 4000);
  const current = poses.playbackKey()!;
  assert.notEqual(current, old);
  poses.startPlayback(old, 4500);
  poses.finishPlayback(old);
  assert.deepEqual(poses.tick(true, 5000), { frame: 7, kind: "tap" });
  poses.startPlayback(current, 5000);
  assert.equal(poses.tick(true, 8100), null);
});

test("native expiry permits completion while input, pause and authority loss stop it", () => {
  const state: DesktopCompanionState = {
    runtimeEpoch: "epoch-a", revision: "1", motionGeneration: "8", phase: "playing",
    pauseReason: null, inputIdleMs: 125_000, activeApp: null, motion: null,
    capabilities: { systemIdle: true, roaming: false, workApps: true,
      launchers: true, fileDrop: false, windowEdges: false },
  };
  assert.equal(allowsCompanionPose(state), true);
  assert.equal(allowsCompanionPose({ ...state, phase: "idle" }), true);
  for (const value of [null, undefined, { ...state, inputIdleMs: 0 },
    { ...state, inputIdleMs: null }, { ...state, phase: "paused" as const },
    { ...state, pauseReason: "workActive" },
    { ...state, capabilities: { ...state.capabilities, systemIdle: false } }])
    assert.equal(allowsCompanionPose(value), false);
  assert.equal(allowsCompanionPose(state, false), false);
  assert.equal(allowsCompanionPose(null, true, true), true);
  assert.equal(allowsCompanionPose(null, false, true), false);
  assert.equal(allowsCompanionPose({ ...state, phase: "paused" }, true, true), false);
});
