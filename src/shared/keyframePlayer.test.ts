import test from "node:test";
import assert from "node:assert/strict";
import { KeyframePlayer, playbackClip, type KeyframeClip } from "../features/pet/keyframePlayer";

const blink: KeyframeClip = {
  id: "idle", fallback: "idle/base", loop: true,
  sequence: [
    { frame: "idle/base", durationMs: 800 },
    { frame: "idle/half-blink", durationMs: 75 },
    { frame: "idle/closed-blink", durationMs: 100 },
    { frame: "idle/half-blink", durationMs: 75 },
  ],
};

test("keyframe deadlines preserve the short blink and exact loop boundaries", () => {
  const clock = new KeyframePlayer();
  assert.deepEqual(clock.start(blink, 100), { frame: "idle/base", nextInMs: 800 });
  assert.deepEqual(clock.read(900), { frame: "idle/half-blink", nextInMs: 75 });
  assert.deepEqual(clock.read(975), { frame: "idle/closed-blink", nextInMs: 100 });
  assert.deepEqual(clock.read(1075), { frame: "idle/half-blink", nextInMs: 75 });
  assert.deepEqual(clock.read(1150), { frame: "idle/base", nextInMs: 800 });
});

test("late callbacks skip missed frames rather than replaying a backlog", () => {
  const clock = new KeyframePlayer();
  clock.start(blink, 0);
  assert.deepEqual(clock.read(1050 * 60 + 900), { frame: "idle/closed-blink", nextInMs: 75 });
  assert.deepEqual(clock.read(1050 * 60 + 975), { frame: "idle/half-blink", nextInMs: 75 });
});

test("replacement, finite completion and invalid clocks fence old playback", () => {
  const clock = new KeyframePlayer();
  clock.start(blink, 0);
  const alert = { ...blink, id: "alert", fallback: "alert/base", loop: false,
    sequence: [{ frame: "alert/accent", durationMs: 300 }] };
  assert.deepEqual(clock.start(alert, 850), { frame: "alert/accent", nextInMs: 300 });
  assert.deepEqual(clock.read(1150), { frame: "alert/base", nextInMs: null });
  assert.throws(() => clock.read(1200), /尚未/);
  clock.start(blink, 100);
  assert.deepEqual(clock.read(99), { frame: "idle/base", nextInMs: null });
  assert.throws(() => clock.read(150), /尚未/);
  assert.throws(() => clock.start({ ...blink, sequence: [] }, 0), /无效/);
  assert.throws(() => clock.start(blink, NaN), /无效/);
});

test("state clips enter once and repeat only the reviewed body range", () => {
  const clock = new KeyframePlayer();
  const clip: KeyframeClip = { id: "active", fallback: "idle/master-front", loop: true,
    loopRange: { start: 1, end: 3 }, sequence: [
      { frame: "entry", durationMs: 100 },
      { frame: "body-a", durationMs: 200 },
      { frame: "body-b", durationMs: 300 },
      { frame: "exit", durationMs: 100 },
    ] };
  assert.equal(clock.start(clip, 0).frame, "entry");
  assert.equal(clock.read(100).frame, "body-a");
  assert.equal(clock.read(300).frame, "body-b");
  assert.deepEqual(clock.read(600), { frame: "body-a", nextInMs: 200 });
  assert.deepEqual(clock.read(100_900), { frame: "body-b", nextInMs: 200 });
  for (const loopRange of [{ start: -1, end: 2 }, { start: 2, end: 2 }, { start: 0, end: 5 }])
    assert.throws(() => clock.start({ ...clip, loopRange }, 0), /无效/);
  assert.throws(() => clock.start({ ...clip, loop: false }, 0), /无效/);
});

test("the 26-step in-place walk preserves repeated steps and its finite return", () => {
  const clock = new KeyframePlayer();
  const sequence = Array.from({ length: 26 }, (_, index) => ({
    frame: `walk/${index % 12}`, durationMs: index === 25 ? 170 : 220,
  }));
  const clip: KeyframeClip = { id: "walk", fallback: "idle/master-front", loop: false, sequence };
  assert.equal(sequence.reduce((sum, step) => sum + step.durationMs, 0), 5670);
  clock.start(clip, 0);
  assert.deepEqual(clock.read(5500), { frame: "walk/1", nextInMs: 170 });
  assert.deepEqual(clock.read(5670), { frame: "idle/master-front", nextInMs: null });
});

test("finite prompts and sleep finish their exit and return without jumping to the static fallback", () => {
  for (const loop of [true, false]) {
    const clip: KeyframeClip = { id: loop ? "alert" : "sleep", fallback: "static-fallback", loop,
      ...(loop ? { loopRange: { start: 1, end: 2 } } : {}), sequence: [
        { frame: "entry", durationMs: 100 }, { frame: "body", durationMs: 1000 },
        { frame: "exit", durationMs: 100 }, { frame: "idle/master-front", durationMs: 100 },
      ] };
    const player = new KeyframePlayer();
    player.start(playbackClip(clip, loop), 0);
    assert.equal(player.read(1100).frame, "exit");
    assert.equal(player.read(1200).frame, "idle/master-front");
    assert.deepEqual(player.read(1300), { frame: "idle/master-front", nextInMs: null });
    assert.equal(clip.fallback, "static-fallback");
  }
  assert.equal(playbackClip(blink), blink);
});

test("window IO pause excludes waiting while preserving the current step's remaining duration", () => {
  const clock = new KeyframePlayer();
  clock.start({ id: "turn", fallback: "front", loop: false, sequence: [
    { frame: "entry", durationMs: 100 }, { frame: "turn", durationMs: 200 },
    { frame: "front", durationMs: 100 },
  ] }, 0);
  const paused = { frame: "turn", nextInMs: 175, stepIndex: 1 };
  assert.deepEqual(clock.pause(125), paused);
  assert.deepEqual(clock.pause(5000), paused, "repeated pause does not shift its origin");
  assert.deepEqual(clock.readPosition(10_000), paused);
  assert.deepEqual(clock.resume(20_000), paused);
  assert.deepEqual(clock.readPosition(20_174), { frame: "turn", nextInMs: 1, stepIndex: 1 });
  assert.deepEqual(clock.readPosition(20_175), { frame: "front", nextInMs: 100, stepIndex: 2 });
  assert.deepEqual(clock.readPosition(20_275), { frame: "front", nextInMs: null, stepIndex: null });
});

test("window pause retains loop-body indices and a replacement cannot resume its old timeline", () => {
  const clock = new KeyframePlayer();
  clock.start({ id: "active", fallback: "front", loop: true, loopRange: { start: 1, end: 3 }, sequence: [
    { frame: "entry", durationMs: 100 }, { frame: "body-a", durationMs: 200 },
    { frame: "body-b", durationMs: 300 }, { frame: "exit", durationMs: 100 },
  ] }, 0);
  const paused = { frame: "body-b", nextInMs: 250, stepIndex: 2 };
  assert.deepEqual(clock.pause(850), paused);
  assert.deepEqual(clock.resume(10_850), paused);
  assert.deepEqual(clock.readPosition(11_100), { frame: "body-a", nextInMs: 200, stepIndex: 1 });
  clock.pause(11_110);
  clock.start(blink, 11_200);
  assert.deepEqual(clock.resume(11_250), { frame: "idle/base", nextInMs: 750, stepIndex: 0 });
  clock.stop();
  assert.throws(() => clock.readPosition(11_300), /尚未/);
});
