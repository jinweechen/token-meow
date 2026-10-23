import assert from "node:assert/strict";
import test from "node:test";
import type { PetPresentation } from "./contracts";
import { VrmPetPresentation, type VrmPetInput } from "../features/pet/vrmPetPresentation";

function fixture(): VrmPetInput {
  const presentation: PetPresentation & { runtimeEpoch: string } = {
    runtimeEpoch: "runtime-a", sourceScopeRevision: "scope-a",
    settings: {}, activity: { activeTurns: 0, uncertainTurns: 0, state: "idle" },
    health: { lastSuccessfulScan: null, lastSourceEvent: null, issues: 0, conflicts: 0 },
    unreadCount: 0, prompts: [],
  };
  return { modelId: "model-a", presentation, ready: true, blocked: false, reducedMotion: false };
}
function prompt(input: VrmPetInput, id: string, now: number, status = "completed", kind = "completion", eligible = true) {
  input.presentation!.prompts = [{ id, status, kind, createdAt: new Date(now).toISOString(), animationEligible: eligible }];
}

test("VRM ignores initial facts, waits for painted acknowledgement, and finishes one finite prompt", () => {
  const owner = new VrmPetPresentation(), input = fixture();
  prompt(input, "boot", 1000);
  assert.equal(owner.observe(input, 1000).promptId, null);
  prompt(input, "fresh", 1100);
  const frame = owner.observe(input, 1100);
  assert.equal(frame.action, "complete");
  assert.equal(frame.promptId, "fresh");
  assert.equal(owner.observe(input, 4000).promptId, "fresh");
  assert.equal(owner.acknowledge(frame.domain, "wrong", 4000), false);
  assert.equal(owner.acknowledge(frame.domain, "fresh", 4000), true);
  assert.equal(owner.acknowledge(frame.domain, "fresh", 4100), false);
  assert.equal(owner.observe(input, 5499).promptId, "fresh");
  assert.equal(owner.observe(input, 5500).promptId, null);
  assert.equal(owner.observe(input, 6500).action, "idle");
});

test("VRM does not acknowledge a prompt that aged out while the model was loading", () => {
  const owner = new VrmPetPresentation(), input = fixture();
  owner.observe(input, 1000);
  prompt(input, "waiting-for-model", 1100);
  const frame = owner.observe(input, 1100);
  assert.equal(frame.promptId, "waiting-for-model");
  assert.equal(owner.observe(input, 11_101).promptId, null);
  assert.equal(owner.acknowledge(frame.domain, "waiting-for-model", 11_101), false);
});

test("VRM source, runtime, model and null transitions baseline arrivals and reject old acknowledgement", () => {
  for (const change of ["source", "runtime", "model", "null"] as const) {
    const owner = new VrmPetPresentation(), input = fixture();
    owner.observe(input, 1000); prompt(input, "old", 1100);
    const old = owner.observe(input, 1100);
    if (change === "source") input.presentation!.sourceScopeRevision = "scope-b";
    if (change === "runtime") (input.presentation as PetPresentation & { runtimeEpoch: string }).runtimeEpoch = "runtime-b";
    if (change === "model") input.modelId = "model-b";
    if (change === "null") { input.presentation = null; input.ready = false; }
    assert.equal(owner.observe(input, 1200).promptId, null);
    assert.equal(owner.acknowledge(old.domain, "old", 1200), false);
    if (change === "null") Object.assign(input, fixture());
    owner.observe(input, 1250);
    prompt(input, "new", 1300);
    assert.equal(owner.observe(input, 1300).promptId, "new");
  }
});

test("VRM hidden, runtime pause, DND and reduced gates advance the watermark without replay", () => {
  for (const kind of ["hidden", "paused", "dnd", "reduced", "loading"] as const) {
    const owner = new VrmPetPresentation(), input = fixture();
    owner.observe(input, 1000); prompt(input, "visible", 1100);
    const old = owner.observe(input, 1100);
    if (kind === "reduced") input.reducedMotion = true;
    else if (kind === "loading") input.ready = false;
    else input.blocked = true;
    prompt(input, "during-pause", 1200);
    assert.equal(owner.observe(input, 1200).promptId, null);
    assert.equal(owner.acknowledge(old.domain, "visible", 1200), false);
    input.reducedMotion = false; input.blocked = false; input.ready = true;
    assert.equal(owner.observe(input, 1300).promptId, null);
    assert.equal(owner.observe(input, 1350).promptId, null);
    prompt(input, "after-resume", 1400);
    assert.equal(owner.observe(input, 1400).promptId, "after-resume");
  }
});

test("VRM delayed React batches created during pause cannot replay after a resume event", () => {
  const owner = new VrmPetPresentation(), input = fixture();
  owner.observe(input, 1000);
  input.blocked = true; owner.observe(input, 1100);
  input.blocked = false; owner.observe(input, 1300);
  prompt(input, "late-paused-batch", 1200);
  assert.equal(owner.observe(input, 1400).promptId, null);
  prompt(input, "fresh-after-resume", 1500);
  assert.equal(owner.observe(input, 1500).promptId, "fresh-after-resume");
});

test("VRM reactions share existing whitelist and cooldown, expire, and never cover active or uncertain facts", () => {
  const owner = new VrmPetPresentation(), input = fixture();
  owner.observe(input, 1000);
  assert.equal(owner.tap(input, 1100, "wink").reaction, "wink");
  assert.equal(owner.observe(input, 2599).reaction, "wink");
  assert.equal(owner.observe(input, 2600).reaction, null);
  assert.equal(owner.tap(input, 2700, "stretch").reaction, null);
  assert.equal(owner.tap(input, 4100, "stretch").reaction, "stretch");
  input.presentation!.activity.activeTurns = 2;
  assert.equal(owner.tap(input, 4200, "affection").reaction, null);
  assert.equal(owner.observe(input, 4201).action, "active");
  input.presentation!.activity.activeTurns = 0; input.presentation!.activity.uncertainTurns = 1;
  assert.equal(owner.tap(input, 8000, "affection").reaction, null);
  assert.equal(owner.observe(input, 8001).action, "alert");
  input.presentation!.activity.uncertainTurns = 0;
  assert.equal(owner.tap(input, 9000, "unknown-pose").reaction, null);
});

test("VRM unknown and budget prompts remain alerts, static restrictions retain actual intent", () => {
  const owner = new VrmPetPresentation(), input = fixture();
  owner.observe(input, 1000); prompt(input, "unknown", 1100, "unknown");
  assert.equal(owner.observe(input, 1100).action, "alert");
  owner.interrupt(); prompt(input, "budget", 1200, "completed", "budget");
  assert.equal(owner.observe(input, 1200).action, "alert");
  input.presentation!.activity.activeTurns = 3; input.reducedMotion = true;
  const staticFrame = owner.observe(input, 1300);
  assert.equal(staticFrame.action, "active"); assert.equal(staticFrame.promptId, null);
  assert.equal(staticFrame.reaction, null); assert.match(staticFrame.text, /3 个回合活动中/);
  owner.dispose(); prompt(input, "after-dispose", 1400);
  assert.equal(owner.observe(input, 1400).promptId, null);
});

test("VRM clock rollback interrupts unacknowledged transient work without fabricating completion", () => {
  const owner = new VrmPetPresentation(), input = fixture();
  owner.observe(input, 1000); prompt(input, "fresh", 1100);
  const frame = owner.observe(input, 1100);
  assert.equal(owner.observe(input, 1050).promptId, null);
  assert.equal(owner.acknowledge(frame.domain, "fresh", 1051), false);
  assert.equal(owner.observe(input, 1200).action, "idle");
});
