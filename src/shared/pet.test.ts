import assert from "node:assert/strict";
import test from "node:test";
import {
  AnimationController,
  ANIMATION_LIMITS,
} from "../features/pet/animationController";
import { presentPet, promptAction } from "../features/pet/presenter";
import { emptySnapshot } from "./demo";
import { PromptWatermark } from "../features/pet/promptWatermark";
import type { InboxEvent } from "./contracts";
import { projectSnapshotPresentation } from "../features/pet/petProjection";
test("bounded prompts deduplicate, expire, and failed results interrupt celebration", () => {
  const controller = new AnimationController();
  assert.equal(
    controller.enqueue(
      { id: "one", status: "completed", createdAt: 1000 },
      1000,
    ),
    true,
  );
  assert.equal(
    controller.enqueue(
      { id: "one", status: "completed", createdAt: 1000 },
      1000,
    ),
    false,
  );
  assert.equal(controller.tick("active", 1000).action, "complete");
  controller.enqueue({ id: "two", status: "failed", createdAt: 1100 }, 1100);
  assert.equal(controller.tick("active", 1100).action, "alert");
  assert.equal(controller.tick("active", 3101).action, "active");
  assert.equal(
    controller.enqueue(
      { id: "late", status: "completed", createdAt: 0 },
      20_000,
    ),
    false,
  );
  const bounded = new AnimationController();
  for (let i = 0; i < 20; i++)
    bounded.enqueue({ id: String(i), status: "unknown", createdAt: 0 }, 0);
  assert.equal(bounded.diagnostics().queued, ANIMATION_LIMITS.queue);
});
test("hidden/reduced/DND suppression clears transient prompts without replay", () => {
  const controller = new AnimationController();
  controller.enqueue({ id: "one", status: "completed", createdAt: 0 }, 0);
  controller.suppress(true);
  assert.equal(controller.tick("active", 1).phase, "static");
  controller.suppress(false);
  assert.equal(controller.tick("active", 2).action, "active");
  controller.destroy();
  assert.equal(
    controller.enqueue(
      { id: "after-dispose", status: "completed", createdAt: 3 },
      3,
    ),
    false,
  );
});
test("prompt animations retain their full clip duration after asset readiness and failures preempt", () => {
  const controller = new AnimationController((action) => action === "complete" ? 3100 : 4200, true);
  controller.enqueue({ id: "done", status: "completed", createdAt: 1000 }, 1000);
  assert.equal(controller.tick("idle", 1000).action, "complete");
  assert.equal(controller.tick("idle", 6000).action, "complete");
  controller.startPlayback("done", 6000);
  controller.startPlayback("done", 7000);
  assert.equal(controller.tick("idle", 9099).action, "complete");
  controller.enqueue({ id: "failed", status: "failed", createdAt: 9100 }, 9100);
  controller.startPlayback("done", 9101); // A stale completion cannot start the replacement.
  controller.startPlayback("failed", 9200);
  assert.equal(controller.tick("idle", 13_399).action, "alert");
  assert.equal(controller.tick("idle", 13_400).action, "idle");
});
test("presenter keeps parallel activity and unknown independent from health", () => {
  const snapshot = emptySnapshot();
  snapshot.activity.activeTurns = 2;
  snapshot.activity.uncertainTurns = 1;
  const presentation = projectSnapshotPresentation(snapshot);
  assert.equal(presentPet(presentation).active, 2);
  assert.equal(presentPet(presentation).action, "active");
  assert.equal(presentPet(presentation).uncertain, 1);
  assert.equal(
    promptAction({ id: "x", status: "unknown", createdAt: 0 }),
    "alert",
  );
});
test("production prompt watermark ignores first load, historical import, result revisions and hidden backlog", () => {
  const watermark = new PromptWatermark();
  const event = (
    id: string,
    eligible = true,
    status = "completed",
  ): InboxEvent => ({
    id,
    status,
    kind: "completion",
    threadId: "thread",
    turnId: "turn",
    title: "",
    read: false,
    createdAt: new Date(1000).toISOString(),
    notifyEligible: eligible,
    animationEligible: eligible,
    notificationState: eligible ? "pending" : "history-only",
  });
  assert.deepEqual(watermark.consume([event("startup")], 1001, false), []);
  assert.deepEqual(
    watermark.consume([event("history", false)], 1002, false),
    [],
  );
  assert.equal(
    watermark.consume([event("live", true, "unknown")], 1003, false).length,
    1,
  );
  assert.deepEqual(
    watermark.consume([event("live", true, "completed")], 1004, false),
    [],
  );
  assert.deepEqual(watermark.consume([event("hidden")], 1005, true), []);
  assert.deepEqual(watermark.consume([event("hidden")], 1006, false), []);
  assert.deepEqual(watermark.consume([event("late")], 20_000, false), []);
});
