import assert from "node:assert/strict";
import test from "node:test";
import type { PetChatOpenRequest, PetChatOpenResult, PetPanelRequest } from "./contracts";
import { demoPetDetails, demoPetPulse } from "./demo";
import {
  PetCardActionController, capturePetCard, type PetCardSnapshot,
} from "./panelNavigation";

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function harness(desktop = true) {
  let now = Date.now();
  const pulse = demoPetPulse();
  const request = { day: pulse.day, runtimeEpoch: pulse.runtimeEpoch, sourceScopeRevision: pulse.sourceScopeRevision };
  const details = demoPetDetails(request);
  details.recentChat = { sourceId: "source-a", threadId: "native-thread-a" };
  const state: PetCardSnapshot = { ready: true, pulse, detailsReady: true, details };
  const events: string[] = [], copied: string[] = [], notices: string[] = [];
  const chatRequests: PetChatOpenRequest[] = [], inboxRequests: PetPanelRequest[] = [];
  const response = deferred<PetChatOpenResult>();
  let clipboardFails = false, refreshes = 0;
  const owner = new PetCardActionController({
    desktop, now: () => now, current: () => state,
    close: () => { events.push("close"); state.detailsReady = false; },
    interrupt: () => events.push("interrupt"),
    busy: (value) => events.push("busy:" + value),
    openInbox: async (value) => { events.push("inbox"); inboxRequests.push(value); },
    openChat: (value) => { events.push("chat"); chatRequests.push(value); return response.promise; },
    copy: async (value) => { if (clipboardFails) throw Error("clipboard denied"); copied.push(value); },
    notify: (value) => notices.push(value),
    refresh: async () => { ++refreshes; },
  });
  owner.setVisible(true);
  return {
    owner, state, request, events, copied, notices, response, chatRequests, inboxRequests,
    now: () => now, time: (value: number) => { now = value; },
    failClipboard: () => { clipboardFails = true; }, refreshes: () => refreshes,
  };
}

test("pet card captures domain and native identity before own close and submits only one busy click", async () => {
  const h = harness();
  const first = h.owner.openChat();
  const duplicate = h.owner.openChat();
  assert.deepEqual(h.events, ["busy:true", "close", "interrupt", "chat"]);
  assert.equal(h.state.detailsReady, false);
  assert.equal(h.chatRequests.length, 1);
  assert.deepEqual(h.chatRequests[0], { petDomain: h.request, chat: { sourceId: "source-a", threadId: "native-thread-a" } });
  h.state.details!.recentChat = { sourceId: "source-a", threadId: "newer-thread" };
  h.response.resolve({ status: "submitted" });
  await Promise.all([first, duplicate]);
  assert.deepEqual(h.copied, []);
  assert.deepEqual(h.notices, ["已请求打开聊天"]);
  assert.equal(h.events.at(-1), "busy:false");
  h.owner.dispose();
});

test("pet inbox captures fresh pulse without details and closes before finite open-panel request", async () => {
  const h = harness(); h.state.detailsReady = false; h.state.details = null;
  await h.owner.openInbox();
  assert.deepEqual(h.events, ["busy:true", "close", "interrupt", "inbox", "busy:false"]);
  assert.deepEqual(h.inboxRequests, [{ destination: "inbox", petDomain: h.request }]);
  assert.deepEqual(h.copied, []);
  h.owner.dispose();
});

test("only a current explicit copyRequired result copies the captured native thread", async () => {
  const h = harness();
  const pending = h.owner.openChat();
  h.response.resolve({ status: "copyRequired", threadId: "native-thread-a", message: "协议暂不可用" });
  await pending;
  assert.deepEqual(h.copied, ["native-thread-a"]);
  assert.match(h.notices[0], /会话标识已复制/);
  h.owner.dispose();
  const wrong = harness();
  const wrongPending = wrong.owner.openChat();
  wrong.response.resolve({ status: "copyRequired", threadId: "different-thread", message: "不可用" });
  await wrongPending;
  assert.deepEqual(wrong.copied, []);
  assert.equal(wrong.refreshes(), 1);
  wrong.owner.dispose();
});

test("ordinary rejected pet-chat IPC never falls back to clipboard", async () => {
  const h = harness();
  const pending = h.owner.openChat();
  h.response.reject(Error("source domain expired"));
  await pending;
  assert.deepEqual(h.copied, []);
  assert.equal(h.refreshes(), 1);
  assert.match(h.notices[0], /请更新/);
  h.owner.dispose();
});

test("clipboard denial exposes the exact validated identity without claiming copy succeeded", async () => {
  const h = harness(); h.failClipboard();
  const pending = h.owner.openChat();
  h.response.resolve({ status: "copyRequired", threadId: "native-thread-a", message: "无法定位" });
  await pending;
  assert.deepEqual(h.copied, []);
  assert.match(h.notices[0], /会话标识：native-thread-a/);
  assert.doesNotMatch(h.notices[0], /已复制/);
  h.owner.dispose();
});

test("hidden/resumed, invalidated scope and disposed StrictMode owners reject delayed chat fallback", async () => {
  for (const revoke of [
    (h: ReturnType<typeof harness>) => { h.owner.setVisible(false); h.owner.setVisible(true); },
    (h: ReturnType<typeof harness>) => { h.owner.invalidate(); },
    (h: ReturnType<typeof harness>) => { h.state.pulse = { ...h.state.pulse!, sourceScopeRevision: "new-scope" }; },
    (h: ReturnType<typeof harness>) => { h.state.ready = false; },
    (h: ReturnType<typeof harness>) => { h.owner.dispose(); },
    (h: ReturnType<typeof harness>) => { h.time(h.now() + 15_001); },
    (h: ReturnType<typeof harness>) => { h.time(h.now() - 1); },
  ]) {
    const h = harness(), pending = h.owner.openChat();
    revoke(h);
    h.response.resolve({ status: "copyRequired", threadId: "native-thread-a", message: "不可用" });
    await pending;
    assert.deepEqual(h.copied, []);
    assert.deepEqual(h.notices, []);
    h.owner.dispose();
  }
});

test("missing metadata, loading, stale or cross-domain details cannot become a chat target", async () => {
  for (const mutate of [
    (h: ReturnType<typeof harness>) => { h.state.details!.recentChat = null; },
    (h: ReturnType<typeof harness>) => { h.state.detailsReady = false; },
    (h: ReturnType<typeof harness>) => { h.state.details!.sourceScopeRevision = "other"; },
    (h: ReturnType<typeof harness>) => { h.state.details!.asOf = new Date(h.now() - 15_001).toISOString(); },
  ]) {
    const h = harness(); mutate(h);
    assert.equal(capturePetCard(h.state, h.now(), true), null);
    await h.owner.openChat();
    assert.equal(h.chatRequests.length, 0);
    assert.deepEqual(h.copied, []);
    h.owner.dispose();
  }
});

test("browser preview never submits native navigation or chat and never copies demo identity", async () => {
  const h = harness(false);
  await h.owner.openChat(); await h.owner.openInbox();
  assert.deepEqual(h.chatRequests, []);
  assert.deepEqual(h.inboxRequests, []);
  assert.deepEqual(h.copied, []);
  assert.deepEqual(h.events, []);
  h.owner.dispose();
});

