import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { ANIMATION_ASSET_LIMITS, planAnimationWindow } from "../features/pet/animationClipRegistry";
import { KeyframePlayer, playbackClip, type KeyframeClip } from "../features/pet/keyframePlayer";

// Read catalog metadata only. This proves the window/clock plan, not browser
// decoding, appearance, native hit regions, or desktop movement qualification.
const catalog = JSON.parse(readFileSync(new URL("../../assets/pets/nekomi/manifest-animation-v12.json",
  import.meta.url), "utf8")) as { frames: { bytes: number }[]; clips: KeyframeClip[] };

test("the complete v12 catalog retains the fixed package and twelve-slot admission budgets", () => {
  assert.equal(catalog.frames.length, 63);
  assert.equal(catalog.clips.length, 16);
  assert.ok(catalog.frames.length <= ANIMATION_ASSET_LIMITS.frames);
  assert.ok(catalog.clips.length <= ANIMATION_ASSET_LIMITS.clips);
  assert.ok(catalog.frames.reduce((sum, frame) => sum + frame.bytes, 0) <= ANIMATION_ASSET_LIMITS.encodedBytes);
  assert.equal(ANIMATION_ASSET_LIMITS.clipFrames, 12);
  for (const clip of catalog.clips) {
    const members = new Set([clip.fallback, ...clip.sequence.map((step) => step.frame)]);
    for (const [index, step] of clip.sequence.entries()) {
      const window = planAnimationWindow(clip, index);
      assert.ok(window.length >= 1 && window.length <= 12);
      assert.equal(window[0], step.frame);
      assert.ok(window.every((id) => members.has(id)));
    }
    assert.deepEqual(planAnimationWindow(clip, null), [clip.fallback], "static mode needs just the reviewed fallback");
  }
});

for (const id of ["walk-left-to-right", "walk-right-to-left"]) {
  test(`${id} keeps all 32 steps, 23 images and 6560ms with bounded-window waits`, () => {
    const clip = catalog.clips.find((item) => item.id === id)!;
    assert.equal(clip.sequence.length, 32);
    assert.equal(new Set(clip.sequence.map((step) => step.frame)).size, 23);
    assert.equal(clip.sequence.reduce((sum, step) => sum + step.durationMs, 0), 6560);
    const player = new KeyframePlayer(), displayed: string[] = [];
    player.start(playbackClip(clip), 0);
    let available = new Set(planAnimationWindow(clip, 0)), now = 0, waits = 0;
    for (const [index, step] of clip.sequence.entries()) {
      let sample = player.readPosition(now);
      assert.equal(sample.stepIndex, index);
      assert.equal(sample.nextInMs, step.durationMs);
      if (!available.has(sample.frame)) {
        const paused = player.pause(now);
        now += 1200;
        available = new Set(planAnimationWindow(clip, paused.stepIndex));
        sample = player.resume(now);
        assert.deepEqual(sample, paused);
        waits++;
      }
      assert.ok(available.has(sample.frame));
      displayed.push(sample.frame);
      now += sample.nextInMs!;
    }
    assert.ok(waits >= 1, "the full turn actually crosses the twelve-image window");
    assert.deepEqual(displayed, clip.sequence.map((step) => step.frame));
    assert.equal(now - waits * 1200, 6560);
    assert.deepEqual(player.readPosition(now), { frame: clip.sequence.at(-1)!.frame,
      nextInMs: null, stepIndex: null });
  });
}
