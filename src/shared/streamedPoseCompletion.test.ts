import assert from "node:assert/strict";
import test from "node:test";
import { PoseController } from "../features/pet/poseController";

test("a streamed finite pose waits for playback completion while fresh activity still cancels it", () => {
  const poses = new PoseController({ idle: [15], tap: [15] }, () => Infinity, true);
  assert.equal(poses.tap(true, 1_000), true);
  const first = poses.playbackKey()!;
  poses.startPlayback(first, 2_000);
  // Waiting for later batches must not consume the remaining reviewed steps.
  assert.deepEqual(poses.tick(true, 30_000), { frame: 15, kind: "tap" });
  poses.finishPlayback(first);
  assert.equal(poses.tick(true, 30_001, false), null);

  assert.equal(poses.tap(true, 31_000), true);
  const current = poses.playbackKey()!;
  poses.startPlayback(current, 32_000);
  poses.finishPlayback(first);
  assert.deepEqual(poses.tick(true, 33_000), { frame: 15, kind: "tap" });
  assert.equal(poses.tick(false, 33_001), null);
});
