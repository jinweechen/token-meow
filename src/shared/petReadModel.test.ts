import assert from "node:assert/strict";
import test from "node:test";
import type { PetCursor, PetDetails, PetDetailsRequest, PetPulse } from "./contracts";
import { PetReadLifecycle, petDay } from "../features/pet/petLifecycle";
import { PromptWatermark } from "../features/pet/promptWatermark";
import { presentPet } from "../features/pet/presenter";
import { projectSnapshotPresentation } from "../features/pet/petProjection";
import { WorkCareSession } from "../features/pet/workCareSession";
import { emptySnapshot } from "./demo";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
let nextRevision = 9007199254740993n;
function pulse(now: number, sequence = "0", baseline = true, patch: Partial<PetPulse> = {}): PetPulse {
  const value: PetPulse = {
    protocolVersion: 1,
    runtimeEpoch: "runtime-a",
    revision: String(++nextRevision),
    day: petDay(now),
    asOf: new Date(now).toISOString(),
    sourceScopeRevision: "scope-a",
    settings: { muted: true, workCareEnabled: true, careTone: "mixed", careIntervalMinutes: 25 },
    activity: { activeTurns: 0, uncertainTurns: 0, state: "idle" },
    health: { lastSuccessfulScan: null, lastSourceEvent: null, issues: 0, conflicts: 0 },
    unreadCount: 0,
    prompts: [],
    promptFeed: {
      baseline,
      cursor: { runtimeEpoch: "runtime-a", sourceScopeRevision: "scope-a", day: petDay(now), sequence },
      dropped: { overflow: 0, expired: 0, invalidTime: 0, future: 0, ineligible: 0, otherDay: 0 },
    },
    ...patch,
  };
  value.promptFeed = { ...value.promptFeed, cursor: {
    runtimeEpoch: value.runtimeEpoch,
    sourceScopeRevision: value.sourceScopeRevision,
    day: value.day,
    sequence,
  } };
  return value;
}
function details(now: number, request: PetDetailsRequest): PetDetails {
  return { protocolVersion: 1, ...request, revision: "9007199254740993", asOf: new Date(now).toISOString(), overview: { totalTokens: null, cacheHitRate: null }, latestTurn: null, recentChat: null };
}

test("details require nullable or complete recent-chat metadata and preserve independent TTFT", async () => {
  for (const chat of [null, { sourceId: "source-a", threadId: "native-thread" }, undefined, { sourceId: "source-a" }, { sourceId: 1, threadId: "thread" }, { sourceId: "source-a", threadId: "" }]) {
    const h = harness();
    h.owner.setVisible(true);
    h.pulses[0].reply.resolve(pulse(h.now()));
    await flush();
    h.owner.setDetailsOpen(true);
    const value = details(h.now(), h.cards[0].request);
    value.latestTurn = { timeToFirstTokenMs: 87 };
    (value as unknown as { recentChat: unknown }).recentChat = chat;
    h.cards[0].reply.resolve(value);
    await flush();
    const candidate = chat as { sourceId?: unknown; threadId?: unknown } | null | undefined;
    const valid = candidate === null || (typeof candidate === "object" && candidate !== null && typeof candidate.sourceId === "string" && typeof candidate.threadId === "string" && candidate.threadId.length > 0);
    assert.equal(h.owner.state().detailsReady, valid);
    if (valid) {
      assert.deepEqual(h.owner.state().details?.recentChat, chat);
      assert.equal(h.owner.state().details?.latestTurn?.timeToFirstTokenMs, 87);
    }
    h.owner.dispose();
  }
});
function harness(initial = Date.UTC(2026, 9, 15, 8)) {
  let now = initial;
  let intervalId = 0;
  const intervals = new Map<number, () => void>();
  const pulses: { cursor: PetCursor | null; reply: ReturnType<typeof deferred<PetPulse>> }[] = [];
  const cards: { request: PetDetailsRequest; reply: ReturnType<typeof deferred<PetDetails>> }[] = [];
  const owner = new PetReadLifecycle({
    now: () => now,
    pulse: (cursor) => {
      const reply = deferred<PetPulse>();
      pulses.push({ cursor, reply });
      return reply.promise;
    },
    details: (request) => {
      const reply = deferred<PetDetails>();
      cards.push({ request, reply });
      return reply.promise;
    },
    intervals: {
      set: (callback) => { intervals.set(++intervalId, callback); return intervalId; },
      clear: (id) => { intervals.delete(id); },
    },
  });
  return { owner, pulses, cards, intervals, now: () => now, clock: (value: number) => { now = value; } };
}
async function flush() { for (let i = 0; i < 6; i++) await Promise.resolve(); }

test("pet starts null, commits only accepted decimal cursors and shares one immutable bounded prompt batch", async () => {
  const h = harness();
  assert.equal(h.owner.state().pulse, null);
  assert.equal(h.owner.state().ready, false);
  h.owner.setVisible(true);
  assert.equal(h.pulses[0].cursor, null);
  assert.equal(h.cards.length, 0);
  h.pulses[0].reply.resolve(pulse(h.now(), "90071992547409930"));
  await flush();
  const watermark = new PromptWatermark();
  watermark.consume(h.owner.state().pulse!.prompts, h.now(), false);
  void h.owner.request();
  assert.equal(h.pulses[1].cursor?.sequence, "90071992547409930");
  const fresh = pulse(h.now(), "90071992547409942", false);
  fresh.prompts = Array.from({ length: 8 }, (_, index) => ({ id: String(index), kind: "completion", status: index ? "completed" : "unknown", createdAt: fresh.asOf, animationEligible: true }));
  fresh.promptFeed.dropped.overflow = 4;
  h.pulses[1].reply.resolve(fresh);
  await flush();
  const accepted = h.owner.state().pulse!;
  assert.equal(accepted.prompts.length, 8);
  assert.ok(Object.isFrozen(accepted.prompts) && Object.isFrozen(accepted.prompts[0]));
  assert.equal(watermark.consume(accepted.prompts, h.now(), false).length, 8);
  assert.equal(h.owner.diagnostics().cursor?.sequence, "90071992547409942");
  assert.equal(h.cards.length, 0);
  h.owner.dispose();
});

test("pet event storms are single-flight; ordinary updates and settings retain cursor, domain mutation rejects a late pulse", async () => {
  const h = harness();
  h.owner.setVisible(true);
  h.pulses[0].reply.resolve(pulse(h.now(), "4"));
  await flush();
  for (let i = 0; i < 30; i++) void h.owner.request();
  assert.equal(h.pulses.length, 2);
  assert.equal(h.pulses[1].cursor?.sequence, "4");
  h.owner.settingsChanged();
  h.pulses[1].reply.resolve(pulse(h.now(), "9", false));
  await flush();
  assert.equal(h.pulses.length, 3);
  assert.equal(h.pulses[2].cursor?.sequence, "4");
  h.owner.domainChanged();
  h.pulses[2].reply.resolve(pulse(h.now(), "12", false));
  await flush();
  assert.equal(h.owner.state().ready, false);
  assert.equal(h.pulses[3].cursor, null);
  assert.equal(h.owner.diagnostics().cursor, null);
  h.pulses[3].reply.resolve(pulse(h.now(), "12", true, { sourceScopeRevision: "after-clear" }));
  await flush();
  assert.equal(h.owner.state().pulse?.sourceScopeRevision, "after-clear");
  assert.equal(h.owner.state().ready, true);
  h.owner.dispose();
});

test("hide/resume and disposal fence in-flight replies and keep exactly one visible interval over30cycles", async () => {
  for (let cycle = 0; cycle < 30; cycle++) {
    const h = harness();
    let changes = 0;
    h.owner.subscribe(() => { changes++; });
    h.owner.setVisible(true);
    assert.equal(h.intervals.size, 1);
    h.owner.setVisible(false);
    assert.equal(h.intervals.size, 0);
    h.owner.setVisible(true);
    assert.equal(h.pulses.length, 1);
    h.pulses[0].reply.resolve(pulse(h.now()));
    await flush();
    assert.equal(h.pulses.length, 2);
    assert.equal(h.pulses[1].cursor, null);
    assert.equal(h.owner.state().pulse, null);
    h.owner.dispose();
    const before = changes;
    h.pulses[1].reply.resolve(pulse(h.now()));
    await flush();
    assert.equal(changes, before);
    assert.equal(h.intervals.size, 0);
    assert.equal(h.owner.state().pulse, null);
  }
});

test("Shanghai midnight, clock rollback and observation gaps require fresh baseline without relabeling old facts", async () => {
  const h = harness(Date.UTC(2026, 9, 15, 15, 59, 59));
  h.owner.setVisible(true);
  h.pulses[0].reply.resolve(pulse(h.now(), "2"));
  await flush();
  void h.owner.request();
  const old = pulse(h.now(), "3", false);
  h.clock(h.now() + 2000);
  h.pulses[1].reply.resolve(old);
  await flush();
  assert.equal(h.owner.state().ready, false);
  assert.equal(h.owner.state().pulse?.day, "2026-10-15");
  assert.equal(h.pulses[2].cursor, null);
  h.pulses[2].reply.resolve(pulse(h.now(), "3"));
  await flush();
  assert.equal(h.owner.state().pulse?.day, "2026-10-16");
  h.clock(h.now() - 1);
  void h.owner.request();
  assert.equal(h.pulses[3].cursor, null);
  h.pulses[3].reply.resolve(pulse(h.now(), "3"));
  await flush();
  h.clock(h.now() + 15001);
  void h.owner.request();
  assert.equal(h.pulses[4].cursor, null);
  assert.equal(h.owner.state().ready, false);
  h.owner.dispose();
  h.pulses[4].reply.resolve(pulse(h.now()));
  await flush();
});

test("failed reads keep the last value stale without advancing cursor or observation time", async () => {
  const h = harness();
  h.owner.setVisible(true);
  h.pulses[0].reply.resolve(pulse(h.now(), "10"));
  await flush();
  const accepted = h.owner.state().pulse;
  h.clock(h.now() + 5000);
  void h.owner.request();
  h.pulses[1].reply.reject(new Error("offline"));
  await flush();
  assert.equal(h.owner.state().pulse, accepted);
  assert.equal(h.owner.state().ready, false);
  assert.equal(h.owner.diagnostics().cursor?.sequence, "10");
  h.clock(h.now() + 10001);
  void h.owner.request();
  assert.equal(h.pulses[2].cursor, null);
  h.pulses[2].reply.resolve(pulse(h.now(), "12"));
  await flush();
  assert.equal(h.owner.state().ready, true);
  h.owner.dispose();
});

test("details are on-demand and single-flight; close/source/runtime changes reject old domain replies", async () => {
  const h = harness();
  h.owner.setVisible(true);
  h.pulses[0].reply.resolve(pulse(h.now()));
  await flush();
  assert.equal(h.cards.length, 0);
  h.owner.setDetailsOpen(true);
  assert.deepEqual(h.cards[0].request, { day: petDay(h.now()), runtimeEpoch: "runtime-a", sourceScopeRevision: "scope-a" });
  h.owner.setDetailsOpen(false);
  h.owner.setDetailsOpen(true);
  assert.equal(h.cards.length, 1);
  h.cards[0].reply.resolve(details(h.now(), h.cards[0].request));
  await flush();
  assert.equal(h.owner.state().detailsReady, false);
  assert.equal(h.cards.length, 2);
  h.owner.domainChanged();
  h.cards[1].reply.resolve(details(h.now(), h.cards[1].request));
  await flush();
  assert.equal(h.owner.state().detailsReady, false);
  h.pulses[1].reply.resolve(pulse(h.now(), "5", true, { runtimeEpoch: "runtime-b", sourceScopeRevision: "source-b" }));
  await flush();
  assert.equal(h.cards[2].request.runtimeEpoch, "runtime-b");
  assert.equal(h.cards[2].request.sourceScopeRevision, "source-b");
  h.cards[2].reply.resolve(details(h.now(), h.cards[2].request));
  await flush();
  assert.equal(h.owner.state().detailsReady, true);
  assert.equal(h.owner.state().details?.overview.totalTokens, null);
  assert.equal(h.owner.state().details?.latestTurn, null);
  h.owner.dispose();
});

test("panel preview is an explicit narrow projection; loading/stale presentation does not assert old activity", () => {
  const snapshot = emptySnapshot();
  snapshot.activity.activeTurns = 2;
  const projected = projectSnapshotPresentation(snapshot);
  assert.deepEqual(Object.keys(projected).sort(), ["settings", "activity", "health", "unreadCount", "prompts", "sourceScopeRevision"].sort());
  assert.equal("sessions" in projected, false);
  assert.equal("turns" in projected, false);
  assert.equal(presentPet(projected).active, 2);
  assert.equal(presentPet(projected, false).active, 0);
  assert.equal(presentPet(projected, false).text, "活动数据待更新");
  assert.equal(presentPet(null, false).text, "正在读取本地活动…");
});

test("care visibility and retained-value updates cannot renew an old activity observation", () => {
  let now = Date.UTC(2026, 9, 15, 8);
  const window = new EventTarget();
  const document = Object.assign(new EventTarget(), { hidden: false });
  const timers = new Map<number, () => void>();
  let id = 0;
  let bubble: { kind: string } | null = null;
  const session = new WorkCareSession({ window, document, now: () => now, setTimeout: (callback) => { timers.set(++id, callback); return id; }, clearTimeout: (value) => { timers.delete(value); }, change: (value) => { bubble = value; } });
  const observed = pulse(now, "0", true, { activity: { activeTurns: 1, uncertainTurns: 0, state: "active" } });
  session.update(null, false);
  session.update(observed, true);
  now += 15001;
  window.dispatchEvent(new CustomEvent("view-visibility", { detail: { visible: true } }));
  session.update(observed, true);
  session.tap();
  assert.equal(bubble, null);
  session.update(pulse(now, "0", true, { activity: observed.activity }), true);
  session.tap();
  assert.equal((bubble as { kind: string } | null)?.kind, "tap-busy");
  session.dispose();
  assert.equal(timers.size, 0);
});

test("pet rejects numeric watermarks and equal revisions without renewing the accepted observation, then accepts a higher revision", async () => {
  const violations: string[] = [];
  for (const variant of ["numeric revision", "numeric cursor", "equal revision"] as const) {
    const h = harness();
    const firstAt = h.now();
    const revision = "9007199254740993";
    h.owner.setVisible(true);
    h.pulses[0].reply.resolve(pulse(firstAt, "10", true, { revision }));
    await flush();
    const accepted = h.owner.state().pulse;
    h.clock(firstAt + 1000);
    void h.owner.request();
    const invalid = pulse(h.now(), "11", false, { revision: "9007199254740994" });
    if (variant === "numeric revision")
      (invalid as unknown as { revision: number }).revision = Number(invalid.revision);
    else if (variant === "numeric cursor")
      (invalid.promptFeed.cursor as unknown as { sequence: number }).sequence = 9007199254740993;
    else {
      invalid.revision = revision;
      invalid.asOf = accepted!.asOf;
    }
    h.pulses[1].reply.resolve(invalid);
    await flush();
    if (h.owner.state().ready) violations.push(`${variant}: retained facts stayed ready`);
    if (h.owner.state().pulse !== accepted) violations.push(`${variant}: stale reply replaced accepted facts`);
    if (h.owner.diagnostics().cursor?.sequence !== "10") violations.push(`${variant}: cursor advanced`);
    // A rejected reply at t+1s cannot extend the t0 observation's 15s freshness.
    h.clock(firstAt + 15501);
    void h.owner.request();
    if (h.pulses[2].cursor !== null) violations.push(`${variant}: old observation lifetime renewed`);
    h.pulses[2].reply.resolve(pulse(h.now(), "12", true, { revision: "9007199254740995" }));
    await flush();
    if (!h.owner.state().ready || h.owner.diagnostics().cursor?.sequence !== "12")
      violations.push(`${variant}: valid higher revision did not recover`);
    h.owner.dispose();
  }
  assert.deepEqual(violations, []);
});
