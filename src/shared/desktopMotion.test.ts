import assert from "node:assert/strict";
import test from "node:test";
import { CompanionMotionPresentation, MAX_RETIRED_MOTION_EPOCHS, validCompanionMotion, type CompanionMotionState } from "../features/companion/motionPresentation";
import { CompanionStateReader, validCompanionState, type DesktopCompanionState } from "./desktopCompanion";
import { allowPetMotionHeartbeat, isPetMotionRenderer, PetMotionReadinessOwner, type MotionReadinessInput, type MotionReadinessPayload } from "../features/companion/useMotionReadiness";
import type { PetPulse } from "./contracts";

const state = (revision: string, generation = "1", epoch = "epoch-a"): CompanionMotionState => ({
  runtimeEpoch: epoch, revision, motionGeneration: generation, phase: "moving",
  capabilities: { roaming: true }, motion: { direction: "right", remainingMs: 8000 },
});

test("motion extension rejects unsupported/invalid moving and requires null motion outside moving", () => {
  assert.equal(validCompanionMotion(state("1")), true);
  for (const remainingMs of [0, -1, 8001, 1.5, NaN, Infinity])
    assert.equal(validCompanionMotion({ ...state("1"), motion: { direction: "left", remainingMs } }), false);
  assert.equal(validCompanionMotion({ ...state("1"), capabilities: { roaming: false } }), false);
  assert.equal(validCompanionMotion({ ...state("1"), motion: null }), false);
  assert.equal(validCompanionMotion({ ...state("1"), motion: { direction: "up", remainingMs: 1000 } }), false);
  for (const phase of ["idle", "paused", "playing"] as const) {
    assert.equal(validCompanionMotion({ ...state("1"), phase }), false);
    assert.equal(validCompanionMotion({ ...state("1"), phase, motion: null, capabilities: { roaming: false } }), true);
  }
  assert.equal(validCompanionMotion({ ...state("-1"), motion: null }), false);
  assert.equal(validCompanionMotion({ ...state("1"), motionGeneration: "01" }), false);
});

test("moving direction/app/revision updates only shorten the earliest finite lease and cannot revive expiry", () => {
  const owner = new CompanionMotionPresentation();
  owner.bindEpoch("epoch-a", 1000);
  owner.observe({ ...state("1"), motion: { direction: "right", remainingMs: 6000 } }, 1000);
  assert.equal(owner.read(1000)?.expiresAt, 7000);
  owner.observe({ ...state("2"), activeApp: { id: "opaque", label: "Editor" },
    motion: { direction: "left", remainingMs: 8000 } }, 2000);
  assert.deepEqual(owner.read(2000), { runtimeEpoch: "epoch-a", motionGeneration: "1", phase: "moving",
    direction: "left", expiresAt: 7000, remainingMs: 5000 });
  owner.observe({ ...state("3"), motion: { direction: "right", remainingMs: 500 } }, 3000);
  assert.equal(owner.read(3499)?.remainingMs, 1);
  assert.equal(owner.read(3500), null);
  assert.equal(owner.observe(state("4"), 3600), false);
  assert.equal(owner.read(3600), null);
});

test("playing retains the 2400 ms fallback, while a different phase gets its own generation/phase identity", () => {
  const owner = new CompanionMotionPresentation();
  owner.bindEpoch("epoch-a", 1000);
  owner.observe({ ...state("1"), phase: "playing", motion: null }, 1000);
  owner.observe({ ...state("2"), phase: "playing", motion: null }, 2000);
  assert.equal(owner.read(3399)?.remainingMs, 1);
  assert.equal(owner.read(3400), null);
  assert.equal(owner.observe({ ...state("3"), phase: "playing", motion: null }, 3500), false);
  assert.equal(owner.observe(state("4"), 3500), true); // Same generation, new phase.
  assert.equal(owner.read(3500)?.expiresAt, 11500);
  assert.equal(owner.observe({ ...state("5"), phase: "playing", motion: null }, 3600), false);
  assert.equal(owner.read(3600), null); // The old playing identity cannot return.
  owner.observe({ ...state("6", "2"), phase: "playing", motion: null }, 4000);
  assert.equal(owner.read(4000)?.expiresAt, 6400);
});

test("idle/pause/explicit invalidation clear leases; stale revisions and generations never overwrite a newer lease", () => {
  const owner = new CompanionMotionPresentation();
  owner.bindEpoch("epoch-a", 0);
  owner.observe(state("10", "4"), 0);
  owner.observe({ ...state("11", "4"), phase: "paused", motion: null }, 100);
  assert.equal(owner.read(100), null);
  assert.equal(owner.observe(state("10", "4"), 200), false);
  assert.equal(owner.observe(state("12", "4"), 200), false);
  owner.observe(state("13", "5"), 300);
  assert.equal(owner.observe(state("14", "4"), 400), false);
  assert.equal(owner.read(400)?.motionGeneration, "5");
  owner.observe({ ...state("15", "5"), phase: "idle", motion: null }, 500);
  assert.equal(owner.read(500), null);
  owner.observe(state("16", "6"), 600);
  owner.clear();
  assert.equal(owner.observe(state("17", "6"), 700), false);
  assert.equal(owner.read(700), null);
});

test("only explicit authoritative epoch binding admits a new domain; late old results and same-epoch refresh cannot renew it", () => {
  const owner = new CompanionMotionPresentation();
  assert.equal(owner.observe(state("1"), 0), false);
  owner.bindEpoch("epoch-a", 0);
  owner.observe(state("100", "90"), 0);
  assert.equal(owner.observe(state("1", "1", "epoch-b"), 100), false);
  assert.equal(owner.read(100)?.runtimeEpoch, "epoch-a");
  owner.bindEpoch("epoch-b", 200);
  owner.observe(state("1", "0", "epoch-b"), 200);
  assert.equal(owner.observe(state("101", "91", "epoch-a"), 300), false);
  assert.equal(owner.bindEpoch("epoch-a", 300), false);
  assert.equal(owner.read(300)?.runtimeEpoch, "epoch-b");
  owner.bindEpoch("epoch-b", 400);
  owner.observe(state("2", "0", "epoch-b"), 400);
  assert.equal(owner.read(400)?.expiresAt, 8200);
  assert.equal(owner.read(8200), null);
  owner.bindEpoch("epoch-b", 8300);
  assert.equal(owner.observe(state("3", "0", "epoch-b"), 8300), false);
});

test("clock rollback and malformed current motion retire the old identity without reviving it on a later sample", () => {
  const owner = new CompanionMotionPresentation();
  owner.bindEpoch("epoch-a", 1000);
  owner.observe(state("1"), 1000);
  assert.equal(owner.read(900), null);
  assert.equal(owner.observe(state("2"), 950), false);
  owner.observe(state("3", "2"), 1000);
  assert.equal(owner.observe({ ...state("4", "2"), motion: { direction: "left", remainingMs: 8001 } }, 1100), false);
  assert.equal(owner.read(1100), null);
  assert.equal(owner.observe(state("5", "2"), 1200), false);
  owner.observe(state("6", "3"), 1300);
  assert.equal(owner.read(NaN), null);
  assert.equal(owner.observe(state("7", "3"), 1400), false);
});

test("after thirty-two retired epochs capacity exhaustion permanently closes this owner without forgetting an old domain", () => {
  assert.equal(MAX_RETIRED_MOTION_EPOCHS, 32);
  const owner = new CompanionMotionPresentation();
  for (let index = 0; index <= MAX_RETIRED_MOTION_EPOCHS; index++) {
    const epoch = `bounded-${index}`, now = index * 10;
    assert.equal(owner.bindEpoch(epoch, now), true);
    assert.equal(owner.observe(state("1", "0", epoch), now), true);
    assert.equal(owner.read(now)?.runtimeEpoch, epoch);
  }
  const lastEpoch = `bounded-${MAX_RETIRED_MOTION_EPOCHS}`;
  assert.equal(owner.bindEpoch("overflow-epoch", 400), false);
  assert.equal(owner.read(400), null);
  // Neither the formerly current domain, a retired domain, a new generation,
  // nor explicit clearing can reopen the exhausted owner.
  assert.equal(owner.bindEpoch(lastEpoch, 401), false);
  assert.equal(owner.observe(state("2", "1", lastEpoch), 401), false);
  for (let index = 0; index < MAX_RETIRED_MOTION_EPOCHS; index++)
    assert.equal(owner.bindEpoch(`bounded-${index}`, 402), false);
  owner.clear();
  assert.equal(owner.bindEpoch("another-epoch", 403), false);
  assert.equal(owner.read(403), null);
  const fresh = new CompanionMotionPresentation();
  assert.equal(fresh.bindEpoch("overflow-epoch", 500), true);
  assert.equal(fresh.observe(state("1", "0", "overflow-epoch"), 500), true);
  assert.equal(fresh.read(500)?.runtimeEpoch, "overflow-epoch");
});

const nativeState = (revision = "1"): DesktopCompanionState => ({
  ...state(revision), pauseReason: null, inputIdleMs: 120000, activeApp: null,
  capabilities: { systemIdle: true, roaming: true, workApps: true, launchers: true, fileDrop: false, windowEdges: false },
  motion: { direction: "left", remainingMs: 2400 },
});
test("complete companion contract requires a supported native segment of 1..2400 ms and null motion otherwise", () => {
  assert.equal(validCompanionState(nativeState()), true);
  for (const remainingMs of [0, -1, 2401, 8000, 0.5, NaN, Infinity])
    assert.equal(validCompanionState({ ...nativeState(), motion: { direction: "right", remainingMs } }), false);
  assert.equal(validCompanionState({ ...nativeState(), motion: { direction: "up", remainingMs: 1 } }), false);
  assert.equal(validCompanionState({ ...nativeState(), capabilities: { ...nativeState().capabilities, roaming: false } }), false);
  for (const phase of ["idle", "paused", "playing"] as const) {
    assert.equal(validCompanionState({ ...nativeState(), phase }), false);
    assert.equal(validCompanionState({ ...nativeState(), phase, motion: null }), true);
  }
  assert.equal(validCompanionState({ ...nativeState(), motion: undefined }), false);
});
test("subscribe-first motion events require the reader snapshot before epoch binding and same-generation revisions cannot extend 2400 ms", () => {
  const reader = new CompanionStateReader(), owner = new CompanionMotionPresentation();
  const newer = nativeState("2");
  assert.equal(reader.event(newer), "refresh");
  assert.equal(owner.observe(newer, 0), false);
  assert.equal(reader.snapshot(nativeState("1")), true);
  owner.bindEpoch(reader.state()!.runtimeEpoch, 100);
  owner.observe(reader.state(), 100);
  assert.equal(owner.read(100)?.expiresAt, 2500);
  assert.equal(reader.event(nativeState("3")), "accepted");
  owner.observe(reader.state(), 1000);
  assert.equal(owner.read(1000)?.expiresAt, 2500);
  assert.equal(owner.read(2500), null);
  assert.equal(reader.event(nativeState("4")), "accepted");
  assert.equal(owner.observe(reader.state(), 2600), false);
});

const eligible = { ready: true, gaitAvailable: true, gaitReady: true, motionGeneration: "1", motionDirection: "right" as const };
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
async function settled() { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); }

test("readiness performs one registration, preserves separate gait flags and reuses a 500 ms caller heartbeat", async () => {
  let now = 0;
  const registrations: string[] = [], sent: MotionReadinessPayload[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: async (epoch) => { registrations.push(epoch); return "opaque-token-a"; },
    send: async (payload) => { sent.push(payload); return true; } });
  owner.update({ ...eligible, gaitReady: false });
  await settled();
  assert.deepEqual(registrations, ["renderer-a"]);
  assert.deepEqual(sent[0], { rendererEpoch: "renderer-a", token: "opaque-token-a", sequence: "1", ready: true, gaitAvailable: true, gaitReady: false,
    motionGeneration: null, motionDirection: null });
  now = 250; owner.update({ ...eligible, gaitReady: false }); await settled();
  assert.equal(sent.length, 1);
  now = 500; owner.update({ ...eligible, gaitReady: false }); await settled();
  assert.equal(sent.length, 2);
  now = 501; owner.update(eligible); await settled();
  assert.equal(sent.length, 3);
  assert.equal(sent[2].sequence, "3");
  assert.equal(sent[2].gaitReady, true);
});
test("at most one readiness request is in flight, false replaces the latest true and blocks resurrection before its acknowledgement", async () => {
  let now = 0, inFlight = 0, maximum = 0;
  const sent: MotionReadinessPayload[] = [], replies: ReturnType<typeof deferred<boolean>>[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: async () => "token", send: (payload) => {
      inFlight++; maximum = Math.max(maximum, inFlight); sent.push(payload);
      const reply = deferred<boolean>(); replies.push(reply);
      return reply.promise.finally(() => { inFlight--; });
    } });
  owner.update(eligible); await settled();
  for (now = 500; now <= 10000; now += 250) owner.update(eligible);
  assert.equal(sent.length, 1);
  owner.update({ ...eligible, visible: false });
  owner.update(eligible); // Cannot replace the queued revocation.
  replies[0].resolve(true); await settled();
  assert.equal(sent.length, 2);
  assert.equal(sent[1].ready, false);
  assert.equal(sent[1].gaitAvailable, true);
  assert.equal(sent[1].gaitReady, false);
  assert.equal(sent[1].motionGeneration, null);
  assert.equal(sent[1].motionDirection, null);
  assert.equal(maximum, 1);
  replies[1].resolve(true); await settled();
  now += 250; owner.update(eligible); await settled();
  assert.equal(sent.length, 3);
  assert.equal(sent[2].ready, true);
  owner.dispose(); replies[2].resolve(true); await settled();
  assert.equal(sent[3].ready, false);
  replies[3].resolve(true); await settled();
});
test("a stale queued heartbeat is downgraded to false instead of renewing the native lease", async () => {
  let now = 0;
  const sent: MotionReadinessPayload[] = [], first = deferred<boolean>();
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: async () => "token", send: (payload) => { sent.push(payload); return sent.length === 1 ? first.promise : Promise.resolve(true); } });
  owner.update(eligible); await settled();
  now = 500; owner.update(eligible);
  now = 2500; first.resolve(true); await settled();
  assert.equal(sent.length, 2);
  assert.equal(sent[1].ready, false);
});
test("losing the displayed gait cannot be overwritten by a later ready frame before native acknowledges the restriction", async () => {
  let now = 0;
  const sent: MotionReadinessPayload[] = [], replies: ReturnType<typeof deferred<boolean>>[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: async () => "token", send: (payload) => { sent.push(payload); const reply = deferred<boolean>(); replies.push(reply); return reply.promise; } });
  owner.update(eligible); await settled();
  now = 100; owner.update({ ...eligible, gaitReady: false });
  now = 101; owner.update(eligible);
  replies[0].resolve(true); await settled();
  assert.equal(sent.length, 2);
  assert.equal(sent[1].ready, false);
  assert.equal(sent[1].gaitAvailable, true);
  assert.equal(sent[1].gaitReady, false);
  replies[1].resolve(true); await settled();
  now = 250; owner.update(eligible); await settled();
  assert.equal(sent[2].gaitReady, true);
  owner.dispose(); replies[2].resolve(true); await settled(); replies[3].resolve(true); await settled();
});
test("late registration replies after dispose send no heartbeat and no automatic replacement registration", async () => {
  let now = 0, registrations = 0;
  const registration = deferred<string>(), sent: MotionReadinessPayload[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: () => { registrations++; return registration.promise; },
    send: async (payload) => { sent.push(payload); return true; } });
  owner.update(eligible);
  now = 250; owner.update(eligible);
  owner.dispose(); owner.update(eligible);
  registration.resolve("late-token"); await settled();
  assert.equal(registrations, 1);
  assert.equal(sent.length, 0);
});
test("a rejected old native token clears authority and re-registers only on the next caller update", async () => {
  let now = 0, registrations = 0;
  const sent: MotionReadinessPayload[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: async () => `token-${++registrations}`,
    send: async (payload) => { sent.push(payload); return sent.length !== 1; } });
  owner.update(eligible); await settled();
  assert.equal(registrations, 1);
  assert.equal(sent.length, 1);
  now = 250; owner.update(eligible); await settled();
  assert.equal(registrations, 2);
  assert.equal(sent.length, 2);
  assert.equal(sent[1].token, "token-2");
  assert.equal(sent[1].sequence, "2");
});
test("hidden, reduced-motion, pause and backwards clocks revoke immediately without a timer", async () => {
  for (const restriction of [{ visible: false }, { reducedMotion: true }, { paused: true }, { ready: false }]) {
    let now = 1000;
    const sent: MotionReadinessPayload[] = [];
    const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
      register: async () => "token", send: async (payload) => { sent.push(payload); return true; } });
    owner.update(eligible); await settled();
    now = 1001; owner.update({ ...eligible, ...restriction }); await settled();
    assert.equal(sent.at(-1)?.ready, false);
  }
  let now = 1000;
  const sent: MotionReadinessPayload[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: async () => "token", send: async (payload) => { sent.push(payload); return true; } });
  owner.update(eligible); await settled(); now = 900; owner.update(eligible); await settled();
  assert.equal(sent.at(-1)?.ready, false);
});
test("local restrictions, pagehide-style revocation and disposal retain known gait capability while closing readiness", async () => {
  for (const restriction of [{ visible: false }, { reducedMotion: true }, { paused: true }, { ready: false }]) {
    let now = 1000;
    const sent: MotionReadinessPayload[] = [];
    const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
      register: async () => "token", send: async (payload) => { sent.push(payload); return true; } });
    owner.update({ ...eligible, ...restriction }); await settled();
    assert.deepEqual({ ready: sent[0].ready, gaitAvailable: sent[0].gaitAvailable, gaitReady: sent[0].gaitReady },
      { ready: false, gaitAvailable: true, gaitReady: false });
    now = 1250; owner.update(eligible); await settled();
    now = 1251; owner.revoke(); await settled();
    assert.equal(sent.at(-1)?.gaitAvailable, true);
    assert.equal(sent.at(-1)?.ready, false);
    assert.equal(sent.at(-1)?.gaitReady, false);
    now = 1500; owner.update(eligible); await settled();
    owner.dispose(); await settled();
    assert.equal(sent.at(-1)?.gaitAvailable, true);
    assert.equal(sent.at(-1)?.ready, false);
    assert.equal(sent.at(-1)?.gaitReady, false);
  }
  const sent: MotionReadinessPayload[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-b", now: () => 0,
    register: async () => "token", send: async (payload) => { sent.push(payload); return true; } });
  owner.update({ ...eligible, ready: false, gaitAvailable: false }); await settled();
  assert.equal(sent[0].gaitAvailable, false); // A new build does not inherit another owner's capability.
});
test("failed registration and synchronous or rejected IPC promises have bounded retries driven only by updates", async () => {
  let now = 0, registrations = 0, sends = 0;
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: async () => { registrations++; if (registrations === 1) throw new Error("no registry"); return "token"; },
    send: () => { sends++; throw new Error("IPC down"); } });
  owner.update(eligible); await settled();
  assert.equal(registrations, 1); assert.equal(sends, 0);
  now = 250; owner.update(eligible); await settled();
  assert.equal(registrations, 2); assert.equal(sends, 1);
  await settled(); assert.equal(registrations, 2); assert.equal(sends, 1);
  now = 500; owner.update(eligible); await settled();
  assert.equal(registrations, 3); assert.equal(sends, 2);
});
test("a motion renderer is admitted only in the non-decorative native pet WebView", () => {
  assert.equal(isPetMotionRenderer(true, false, "?window=pet"), true);
  assert.equal(isPetMotionRenderer(true, false, "?demo=1&window=pet"), true);
  for (const [desktop, decorative, search] of [[false, false, "?window=pet"], [true, true, "?window=pet"],
    [true, false, ""], [true, false, "?window=panel"], [true, false, "?window=pets"]] as const)
    assert.equal(isPetMotionRenderer(desktop, decorative, search), false);
});
test("the visual heartbeat only admits fresh idle presentation in the current scope without work, prompts or taps", () => {
  const now = Date.parse("2026-10-17T04:00:00Z");
  const pulse: PetPulse = {
    protocolVersion: 1, runtimeEpoch: "runtime", revision: "1", day: "2026-10-17", asOf: new Date(now).toISOString(),
    sourceScopeRevision: "scope", settings: {}, activity: { activeTurns: 0, uncertainTurns: 0, state: "idle" },
    health: { lastSuccessfulScan: null, lastSourceEvent: null, issues: 0, conflicts: 0 }, unreadCount: 0, prompts: [],
    promptFeed: { baseline: true, cursor: { runtimeEpoch: "runtime", sourceScopeRevision: "scope", day: "2026-10-17", sequence: "0" },
      dropped: { overflow: 0, expired: 0, invalidTime: 0, future: 0, ineligible: 0, otherDay: 0 } },
  };
  const local = { ready: true, paused: false, sourceScopeRevision: "scope", hasPrompt: false, tap: false,
    action: "idle" as const, interactionBlocked: false };
  assert.equal(allowPetMotionHeartbeat(pulse, local, now), true);
  for (const restriction of [{ ready: false }, { paused: true }, { sourceScopeRevision: "other" }, { sourceScopeRevision: null },
    { hasPrompt: true }, { tap: true }, { action: "active" as const }, { interactionBlocked: true }])
    assert.equal(allowPetMotionHeartbeat(pulse, { ...local, ...restriction }, now), false);
  for (const activity of [{ activeTurns: 1, uncertainTurns: 0, state: "active" }, { activeTurns: 0, uncertainTurns: 1, state: "uncertain" }])
    assert.equal(allowPetMotionHeartbeat({ ...pulse, activity }, local, now), false);
  for (const settings of [{ petVisible: false }, { reducedMotion: true }, { doNotDisturb: true }, { effectiveDoNotDisturb: true }])
    assert.equal(allowPetMotionHeartbeat({ ...pulse, settings }, local, now), false);
  assert.equal(allowPetMotionHeartbeat({ ...pulse, asOf: new Date(now - 15001).toISOString() }, local, now), false);
  assert.equal(allowPetMotionHeartbeat({ ...pulse, asOf: new Date(now + 1001).toISOString() }, local, now), false);
  assert.equal(allowPetMotionHeartbeat({ ...pulse, asOf: "bad" }, local, now), false);
  assert.equal(allowPetMotionHeartbeat({ ...pulse, runtimeEpoch: "" }, local, now), false);
  assert.equal(allowPetMotionHeartbeat(null, local, now), false);
});
test("stable false readiness renews only on 500 ms caller ticks and preserves capability without reopening permission", async () => {
  let now = 0;
  const sent: MotionReadinessPayload[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: async () => "token", send: async (payload) => { sent.push(payload); return true; } });
  const quiet = { ...eligible, ready: false };
  owner.update(quiet); await settled();
  for (now = 250; now <= 2500; now += 250) { owner.update(quiet); await settled(); }
  assert.equal(sent.length, 6); // 0, 500, 1000, 1500, 2000, 2500
  assert.equal(sent.every((value) => !value.ready && value.gaitAvailable && !value.gaitReady), true);
  assert.deepEqual(sent.map((value) => value.sequence), ["1", "2", "3", "4", "5", "6"]);
});
test("generation and direction changes emit immediate identity-bound heartbeats before the ordinary 500 ms interval", async () => {
  let now = 0;
  const sent: MotionReadinessPayload[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => now,
    register: async () => "token", send: async (payload) => { sent.push(payload); return true; } });
  owner.update(eligible); await settled();
  now = 1; owner.update({ ...eligible, motionGeneration: "2" }); await settled();
  now = 2; owner.update({ ...eligible, motionGeneration: "2", motionDirection: "left" }); await settled();
  assert.deepEqual(sent.map((value) => [value.motionGeneration, value.motionDirection, value.gaitReady]),
    [["1", "right", true], ["2", "right", true], ["2", "left", true]]);
  now = 3; owner.revoke(); await settled();
  assert.equal(sent[3].ready, false);
  assert.equal(sent[3].gaitReady, false);
  assert.equal(sent[3].gaitAvailable, true);
  assert.equal(sent[3].motionGeneration, null);
  assert.equal(sent[3].motionDirection, null);
});
test("gait readiness without a complete canonical native generation and direction closes permission and clears both identity fields", async () => {
  for (const invalid of [
    { motionGeneration: undefined }, { motionGeneration: null }, { motionGeneration: "" }, { motionGeneration: "01" },
    { motionGeneration: "-1" }, { motionGeneration: "1.5" }, { motionGeneration: "18446744073709551616" },
    { motionDirection: undefined }, { motionDirection: null }, { motionDirection: "up" },
  ]) {
    const sent: MotionReadinessPayload[] = [];
    const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => 0,
      register: async () => "token", send: async (payload) => { sent.push(payload); return true; } });
    owner.update({ ...eligible, ...invalid } as MotionReadinessInput); await settled();
    assert.equal(sent.length, 1);
    assert.equal(sent[0].ready, false);
    assert.equal(sent[0].gaitReady, false);
    assert.equal(sent[0].gaitAvailable, true);
    assert.equal(sent[0].motionGeneration, null);
    assert.equal(sent[0].motionDirection, null);
  }
  const sent: MotionReadinessPayload[] = [];
  const owner = new PetMotionReadinessOwner({ rendererEpoch: "renderer-a", now: () => 0,
    register: async () => "token", send: async (payload) => { sent.push(payload); return true; } });
  owner.update({ ...eligible, motionGeneration: "18446744073709551615", motionDirection: "left" }); await settled();
  assert.equal(sent[0].gaitReady, true);
  assert.equal(sent[0].motionGeneration, "18446744073709551615");
});
