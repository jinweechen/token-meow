import assert from "node:assert/strict";
import test from "node:test";
import { emptySnapshot } from "./demo";
import type { PetPresentation } from "./contracts";
import { projectSnapshotPresentation } from "../features/pet/petProjection";
import {
  CARE_LIMITS,
  WorkCareController,
} from "../features/pet/workCareController";
import { WorkCareSession } from "../features/pet/workCareSession";

function fixture() {
  const snapshot = emptySnapshot();
  snapshot.sources = [
    {
      id: "source-a",
      root: "C:/secret/project",
      enabled: true,
      clientKind: "codex",
      runtime: "可用",
    },
  ];
  snapshot.primarySourceId = "source-a";
  return projectSnapshotPresentation(snapshot);
}
function event(
  snapshot: PetPresentation,
  id: string,
  status: string,
  time: number,
  kind = "completion",
  eligible = true,
) {
  snapshot.prompts = [
    {
      id,
      status,
      kind,
      createdAt: new Date(time).toISOString(),
      animationEligible: eligible,
    },
  ];
}
test("work care cold-start/history/revision/source changes consume facts without replay or private text", () => {
  const owner = new WorkCareController(),
    snapshot = fixture();
  snapshot.activity.activeTurns = 1;
  event(snapshot, "boot", "completed", 1000);
  assert.equal(owner.observe(snapshot, false, false, 1000), null);
  assert.equal(owner.observe(snapshot, true, false, 1001), null);
  event(snapshot, "history", "completed", 1002, "completion", false);
  assert.equal(owner.observe(snapshot, true, false, 1002), null);
  event(snapshot, "unknown", "unknown", 1003);
  const first = owner.observe(snapshot, true, false, 1003)!;
  assert.equal(first.kind, "unknown");
  assert.ok(!first.text.includes("PRIVATE") && !first.text.includes("secret"));
  owner.dismiss();
  event(snapshot, "unknown", "completed", 1004);
  assert.equal(owner.observe(snapshot, true, false, 1004), null);
  snapshot.sourceScopeRevision = "source-identity-changed";
  event(snapshot, "source-change", "completed", 1005);
  assert.equal(owner.observe(snapshot, true, false, 1005), null);
  snapshot.activity.activeTurns = 0;
  owner.observe(snapshot, true, false, 1006);
  owner.destroy();
  snapshot.activity.activeTurns = 1;
  assert.equal(owner.observe(snapshot, true, false, 1007), null);
});
test("work care real starts/completion/failure/budget have one bounded bubble and mixed tones alternate only when shown", () => {
  const owner = new WorkCareController(),
    snapshot = fixture();
  owner.observe(snapshot, true, false, 1000);
  snapshot.activity.activeTurns = 1;
  const start = owner.observe(snapshot, true, false, 1100)!;
  assert.equal(start.kind, "start");
  assert.equal(start.tone, "gentle");
  event(snapshot, "complete", "completed", 1200);
  const complete = owner.observe(snapshot, true, false, 1200)!;
  assert.equal(complete.kind, "complete");
  assert.equal(complete.tone, "playful");
  assert.equal(owner.tap(snapshot.settings, true, 1201)?.id, complete.id);
  event(snapshot, "failed", "failed", 1300);
  const failed = owner.observe(snapshot, true, false, 1300)!;
  assert.equal(failed.kind, "failed");
  assert.equal(failed.tone, "gentle");
  event(snapshot, "budget-drop", "threshold", 1400, "budget");
  assert.equal(owner.observe(snapshot, true, false, 1400)?.id, failed.id);
  assert.equal(owner.current(1300 + CARE_LIMITS.lifetime), null);
  assert.equal(owner.current(1300 + CARE_LIMITS.lifetime + 1), null);
  for (let now = 10000; now <= 70000; now += 10000)
    owner.observe(snapshot, true, false, now);
  event(snapshot, "budget-fresh", "threshold", 71000, "budget");
  const budget = owner.observe(snapshot, true, false, 71000)!;
  assert.equal(budget.kind, "budget");
  assert.equal(budget.tone, "playful");
  assert.ok(budget.text.includes("软预算"));
});
test("ongoing care requires continuously fresh global activity observations; gaps/hidden/disable reset it, reduced motion may retain text", () => {
  const owner = new WorkCareController(),
    snapshot = fixture();
  snapshot.activity.activeTurns = 1;
  snapshot.settings.reducedMotion = true;
  owner.observe(snapshot, true, false, 0);
  for (let now = 10000; now < 25 * 60000; now += 10000)
    assert.equal(owner.observe(snapshot, true, false, now), null);
  const ongoing = owner.observe(snapshot, true, false, 25 * 60000)!;
  assert.equal(ongoing.kind, "ongoing");
  assert.ok(
    ongoing.text.includes("回合") && !ongoing.text.includes("连续工作"),
  );
  assert.equal(owner.observe(snapshot, true, true, 25 * 60000 + 1), null);
  assert.equal(owner.observe(snapshot, true, false, 25 * 60000 + 2), null);
  assert.equal(owner.observe(snapshot, true, false, 50 * 60000 + 2), null);
  snapshot.settings.workCareEnabled = false;
  assert.equal(owner.observe(snapshot, true, false, 50 * 60000 + 3), null);
  assert.equal(owner.tap(snapshot.settings, true, 50 * 60000 + 4), null);
  snapshot.settings.workCareEnabled = true;
  assert.equal(owner.observe(snapshot, true, false, 50 * 60000 + 5), null);
});
test("manual care is cooled down, preserves busy/uncertain facts, and never resumes suppressed completion", () => {
  const owner = new WorkCareController(),
    snapshot = fixture();
  owner.observe(snapshot, true, false, 1000);
  const first = owner.tap(snapshot.settings, true, 1001)!;
  assert.equal(first.kind, "tap");
  owner.dismiss();
  assert.equal(owner.tap(snapshot.settings, true, 1002), null);
  snapshot.activity.uncertainTurns = 1;
  owner.observe(snapshot, true, false, 4002);
  assert.equal(owner.tap(snapshot.settings, true, 4002)?.kind, "tap-uncertain");
  owner.observe(snapshot, true, true, 5000);
  event(snapshot, "hidden", "completed", 5001);
  owner.observe(snapshot, true, true, 5001);
  assert.equal(owner.observe(snapshot, true, false, 5002), null);
  snapshot.activity.activeTurns = 1;
  owner.observe(snapshot, true, false, 5003);
  owner.dismiss();
  assert.equal(owner.tap(snapshot.settings, true, 8004)?.kind, "tap-busy");
  owner.dismiss();
  event(snapshot, "old", "completed", 0);
  assert.equal(owner.observe(snapshot, true, false, 20000), null);
});
test("care expires and stops saying a turn is running after activity stops; obscured input consumes no mixed tone", () => {
  const owner = new WorkCareController(),
    snapshot = fixture();
  owner.observe(snapshot, true, false, 1000);
  snapshot.activity.activeTurns = 1;
  assert.equal(owner.observe(snapshot, true, false, 1001)?.tone, "gentle");
  snapshot.activity.activeTurns = 0;
  assert.equal(owner.observe(snapshot, true, false, 1002), null);
  owner.observe(snapshot, true, true, 1003);
  event(snapshot, "obscured", "completed", 1004);
  owner.observe(snapshot, true, true, 1004);
  owner.observe(snapshot, true, false, 1005);
  assert.equal(owner.tap(snapshot.settings, true, 4006)?.tone, "playful");
  assert.equal(owner.current(4006 + CARE_LIMITS.lifetime), null);
});
test("production work-care session owns only one finite timeout; 30 hide/restore/dispose cycles return listeners and timers to baseline", () => {
  class ObservedTarget extends EventTarget {
    count = 0;
    override addEventListener(
      ...args: Parameters<EventTarget["addEventListener"]>
    ) {
      this.count++;
      super.addEventListener(...args);
    }
    override removeEventListener(
      ...args: Parameters<EventTarget["removeEventListener"]>
    ) {
      this.count--;
      super.removeEventListener(...args);
    }
  }
  const window = new ObservedTarget(),
    document = Object.assign(new ObservedTarget(), { hidden: false }),
    timers = new Map<number, () => void>();
  let now = 1000,
    serial = 0,
    changes = 0;
  const host = {
    window,
    document,
    now: () => now,
    setTimeout: (callback: () => void) => {
      timers.set(++serial, callback);
      return serial;
    },
    clearTimeout: (id: number) => {
      timers.delete(id);
    },
    change: () => {
      changes++;
    },
  };
  for (let cycle = 0; cycle < 30; cycle++) {
    const session = new WorkCareSession(host),
      snapshot = fixture();
    session.update(snapshot, true);
    session.tap();
    assert.equal(timers.size, 1);
    window.dispatchEvent(
      new CustomEvent("view-visibility", { detail: { visible: false } }),
    );
    assert.equal(timers.size, 0);
    event(snapshot, "hidden-" + cycle, "completed", now);
    session.update(snapshot, true);
    window.dispatchEvent(
      new CustomEvent("view-visibility", { detail: { visible: true } }),
    );
    assert.equal(timers.size, 0);
    now += 4000;
    // Recovery needs an actual accepted Pulse observation, not a visibility event.
    session.update({ ...snapshot, prompts: [] }, true);
    session.tap();
    assert.equal(timers.size, 1);
    snapshot.settings.doNotDisturb = true;
    session.update(snapshot, true);
    assert.equal(timers.size, 0);
    session.dispose();
    session.dispose();
    assert.equal(window.count, 0);
    assert.equal(document.count, 0);
    assert.equal(timers.size, 0);
    const before = changes;
    session.tap();
    assert.equal(changes, before);
  }
});
test("stale manual care suppresses retained busy/uncertain facts and clock rollback until a fresh observation baseline", () => {
  for (const activity of [
    { activeTurns: 1, uncertainTurns: 0 },
    { activeTurns: 0, uncertainTurns: 1 },
  ]) {
    const owner = new WorkCareController(),
      snapshot = fixture();
    Object.assign(snapshot.activity, activity);
    owner.observe(snapshot, true, false, 1000);
    assert.equal(
      owner.tap(snapshot.settings, true, 1001)?.kind,
      activity.activeTurns ? "tap-busy" : "tap-uncertain",
    );
    assert.equal(owner.tap(snapshot.settings, true, 16001), null);
    assert.equal(owner.current(16001), null);
    assert.equal(owner.tap(snapshot.settings, true, 16002), null);
    assert.equal(owner.observe(snapshot, true, false, 16003), null);
    assert.equal(
      owner.tap(snapshot.settings, true, 16004)?.kind,
      activity.activeTurns ? "tap-busy" : "tap-uncertain",
    );
  }
  const owner = new WorkCareController(),
    snapshot = fixture();
  snapshot.activity.activeTurns = 1;
  owner.observe(snapshot, true, false, 20000);
  assert.equal(owner.tap(snapshot.settings, true, 19999), null);
  snapshot.activity.activeTurns = 0;
  assert.equal(owner.observe(snapshot, true, false, 20001), null);
  assert.equal(owner.tap(snapshot.settings, true, 20002)?.kind, "tap");
});
