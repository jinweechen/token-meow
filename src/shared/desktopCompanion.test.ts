import test from "node:test";
import { PoseController } from "../features/pet/poseController";
import assert from "node:assert/strict";
import { CompanionStateReader, companionText, validCompanionState, type DesktopCompanionState } from "./desktopCompanion";
const state = (revision: string, runtimeEpoch = "epoch-a"): DesktopCompanionState => ({ runtimeEpoch, revision, phase: "idle", pauseReason: null, inputIdleMs: 120000, activeApp: null, motionGeneration: "0", motion: null, capabilities: { systemIdle: true, roaming: false, workApps: true, launchers: true, fileDrop: false, windowEdges: false } });
test("subscribe/fetch race retains a newer same-epoch event", () => {
  const reader = new CompanionStateReader();
  assert.equal(reader.event(state("12")), "refresh");
  assert.equal(reader.snapshot(state("10")), true);
  assert.equal(reader.state()?.revision, "12");
  assert.equal(reader.event(state("11")), "ignored");
});
test("foreign epochs require an authoritative fetch and cannot roll state back", () => {
  const reader = new CompanionStateReader();
  reader.snapshot(state("20"));
  assert.equal(reader.event(state("1", "epoch-b")), "refresh");
  assert.equal(reader.state()?.runtimeEpoch, "epoch-a");
  reader.snapshot(state("2", "epoch-b"));
  assert.equal(reader.state()?.runtimeEpoch, "epoch-b");
  assert.equal(reader.state()?.revision, "2");
});
test("malformed and duplicate revisions are ignored", () => {
  const reader = new CompanionStateReader();
  reader.snapshot(state("9007199254740993"));
  assert.equal(reader.event(state("9007199254740994")), "accepted");
  assert.equal(reader.event(state("9007199254740994")), "ignored");
  assert.equal(reader.event(state("-1")), "ignored");
  assert.equal(validCompanionState({ ...state("1"), inputIdleMs: NaN }), false);
  assert.equal(validCompanionState({}), false);
  assert.equal(validCompanionState(null), false);
});
test("desktop context labels do not claim ledger activity and privacy hides app labels", () => {
  const value = { ...state("1"), activeApp: { id: "opaque-app", label: "Editor" } };
  assert.equal(companionText(value, true), "正在陪你工作");
  assert.equal(companionText(value), "陪你使用 Editor");
  assert.equal(companionText({ ...value, phase: "paused", pauseReason: "userActive" }), "陪你使用 Editor");
  assert.equal(companionText({ ...value, phase: "paused", pauseReason: "doNotDisturb" }), "免打扰中");
  assert.equal(companionText({ ...value, phase: "moving" }), "桌面活动中");
});
test("native idle ownership suppresses autonomous poses while preserving a manual tap", () => {
  const poses = new PoseController({ idle: [6], tap: [7] });
  assert.equal(poses.tick(true, 1000, false), null);
  assert.equal(poses.tick(true, 120000, false), null);
  assert.equal(poses.tap(true, 120001), true);
  assert.deepEqual(poses.tick(true, 120002, false), { frame: 7, kind: "tap" });
  assert.equal(poses.tick(false, 120003, false), null);
});

test("capability payloads are checked and disabled idle play does not promise future play", () => {
  assert.equal(validCompanionState({ ...state("1"), capabilities: undefined }), false);
  assert.equal(validCompanionState({ ...state("1"), capabilities: { ...state("1").capabilities, systemIdle: "true" } }), false);
  assert.equal(companionText({ ...state("1"), pauseReason: "disabled" }), "空闲活动已关闭");
  assert.equal(companionText({ ...state("1"), phase: "paused", pauseReason: "systemBusy" }), "系统当前不宜打扰，暂时休息");
});
