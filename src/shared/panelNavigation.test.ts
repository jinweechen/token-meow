import assert from "node:assert/strict";
import test from "node:test";
import type { PanelNavigationReply, PanelNavigationRequest } from "./contracts";
import {
  PanelNavigationController, parsePanelNavigationReply, type PanelNavigationIntent,
} from "./panelNavigation";

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const domain = { day: "2026-10-17", runtimeEpoch: "runtime", sourceScopeRevision: "scope" };
const large = "9007199254740993";
function reply(revision = large, remainingMs = 5000): PanelNavigationReply {
  return { watermark: revision, pending: { destination: "inbox", revision, petDomain: { ...domain }, remainingMs } };
}
async function flush() { for (let i = 0; i < 12; ++i) await Promise.resolve(); }
function harness() {
  let now = 1000, removals = 0, applyAccepted = true;
  const listener = deferred<() => void>();
  let callback = () => {};
  const calls: { request: PanelNavigationRequest; reply: ReturnType<typeof deferred<PanelNavigationReply>> }[] = [];
  const applied: PanelNavigationIntent[] = [], errors: unknown[] = [];
  const owner = new PanelNavigationController({
    now: () => now,
    listen: (wake) => { callback = wake; return listener.promise; },
    request: (request) => {
      const response = deferred<PanelNavigationReply>();
      calls.push({ request, reply: response });
      return response.promise;
    },
    apply: (intent) => { applied.push(intent); return applyAccepted; },
    error: (error) => errors.push(error),
  });
  return {
    owner, calls, applied, errors,
    ready: () => listener.resolve(() => { ++removals; }),
    wake: () => callback(), time: (value: number) => { now = value; },
    removals: () => removals,
    rejectApply: () => { applyAccepted = false; },
    acceptApply: () => { applyAccepted = true; },
  };
}

test("panel navigation subscribes before initial pull and releases a late StrictMode subscription", async () => {
  const old = harness();
  old.owner.setVisible(true);
  old.wake();
  assert.equal(old.calls.length, 0);
  old.owner.dispose();
  old.ready();
  await flush();
  assert.equal(old.removals(), 1);
  assert.equal(old.calls.length, 0);
  const current = harness();
  current.owner.setVisible(true);
  current.ready();
  await flush();
  assert.equal(current.calls.length, 1);
  current.calls[0].reply.resolve(reply());
  await flush();
  assert.equal(current.applied.length, 1);
  assert.equal(old.applied.length, 0);
  current.owner.dispose();
});

test("hidden mailbox wakes coalesce into one visible pull, apply alone never ACKs, duplicate wake never reapplies", async () => {
  const h = harness();
  h.ready();
  await flush();
  h.wake(); h.wake();
  assert.equal(h.calls.length, 0);
  h.owner.setVisible(true);
  await flush();
  h.wake(); h.wake(); h.wake();
  assert.equal(h.calls.length, 1);
  h.calls[0].reply.resolve(reply());
  await flush();
  assert.equal(h.applied.length, 1);
  assert.equal(h.calls.length, 2);
  assert.deepEqual(h.calls.map((x) => x.request), [{ action: "peek" }, { action: "peek" }]);
  h.calls[1].reply.resolve(reply());
  await flush();
  assert.equal(h.applied.length, 1);
  assert.equal(h.owner.commit(large), true);
  await flush();
  assert.deepEqual(h.calls[2].request, { action: "ack", revision: large });
  assert.equal(h.owner.commit(large), false);
  h.calls[2].reply.resolve({ watermark: large, pending: null });
  await flush();
  h.wake();
  h.calls[3].reply.resolve(reply());
  await flush();
  assert.equal(h.applied.length, 1);
  h.owner.dispose();
});

test("decimal ordering survives old ACK response while a newer single slot is delivered", async () => {
  const h = harness(); h.owner.setVisible(true); h.ready(); await flush();
  h.calls[0].reply.resolve(reply()); await flush();
  h.owner.commit(large); await flush();
  h.wake();
  const next = String(BigInt(large) + 1n);
  h.calls[1].reply.resolve(reply(next)); await flush();
  assert.equal(h.applied.length, 2);
  assert.equal(h.applied[1].revision, next);
  assert.equal(h.owner.commit(large), false);
  assert.equal(h.calls.length, 3);
  h.calls[2].reply.resolve(reply(next)); await flush();
  assert.equal(h.applied.length, 2);
  assert.equal(h.owner.commit(next), true); await flush();
  assert.deepEqual(h.calls[3].request, { action: "ack", revision: next });
  h.owner.dispose();
});

test("manual navigation invalidates late peek before cancel settles and returned watermark fences older replies", async () => {
  const h = harness(); h.owner.setVisible(true); h.ready(); await flush();
  h.owner.cancel();
  h.calls[0].reply.resolve(reply()); await flush();
  assert.equal(h.applied.length, 0);
  assert.deepEqual(h.calls[1].request, { action: "cancel" });
  const cancelled = String(BigInt(large) + 1n);
  h.calls[1].reply.resolve({ watermark: cancelled, pending: null }); await flush();
  h.wake(); h.calls[2].reply.resolve(reply()); await flush();
  assert.equal(h.applied.length, 0);
  h.wake(); h.calls[3].reply.resolve(reply(String(BigInt(cancelled) + 1n))); await flush();
  assert.equal(h.applied.length, 1);
  h.owner.dispose();
});

test("peek latency and duplicate reads cannot renew the original local application deadline", async () => {
  const late = harness(); late.owner.setVisible(true); late.ready(); await flush();
  late.time(1050); late.calls[0].reply.resolve(reply(large, 40)); await flush();
  assert.equal(late.applied.length, 0);
  late.owner.dispose();
  const h = harness(); h.owner.setVisible(true); h.ready(); await flush();
  h.calls[0].reply.resolve(reply(large, 100)); await flush();
  assert.equal(h.applied[0].deadline, 1100);
  h.time(1050); h.wake();
  h.calls[1].reply.resolve(reply(large, 100)); await flush();
  assert.equal(h.applied.length, 1);
  h.time(1100);
  assert.equal(h.owner.commit(large), false);
  assert.equal(h.calls.length, 2);
  h.owner.dispose();
});

test("visibility suspension and disposed owners reject late peek without reopening or ACK", async () => {
  const h = harness(); h.owner.setVisible(true); h.ready(); await flush();
  h.owner.setVisible(false);
  h.calls[0].reply.resolve(reply()); await flush();
  assert.equal(h.applied.length, 0);
  h.time(1100); h.owner.setVisible(true); await flush();
  h.calls[1].reply.resolve(reply(large, 20)); await flush();
  assert.equal(h.applied.length, 1);
  h.owner.dispose();
  assert.equal(h.owner.commit(large), false);
  h.wake(); await flush();
  assert.equal(h.calls.length, 2);
  assert.equal(h.removals(), 1);
});

test("resume retains the earliest deadline and a queued ACK cannot outlive it", async () => {
  const h = harness(); h.owner.setVisible(true); h.ready(); await flush();
  h.calls[0].reply.resolve(reply(large, 100)); await flush();
  h.time(1050); h.owner.setVisible(false); h.owner.setVisible(true); await flush();
  h.time(1110); h.calls[1].reply.resolve(reply(large, 100)); await flush();
  assert.equal(h.applied.length, 1);
  assert.equal(h.owner.commit(large), false);
  h.owner.dispose();

  const queued = harness(); queued.owner.setVisible(true); queued.ready(); await flush();
  queued.calls[0].reply.resolve(reply(large, 100)); await flush();
  queued.wake();
  assert.equal(queued.owner.commit(large), true);
  queued.time(1100); queued.calls[1].reply.resolve(reply(large, 100)); await flush();
  assert.deepEqual(queued.calls.map((x) => x.request), [{ action: "peek" }, { action: "peek" }]);
  queued.owner.dispose();
});

test("in-flight slot retry requests at most one automatic follow-up per external wake", async () => {
  const h = harness(); h.owner.setVisible(true); h.ready(); await flush();
  h.calls[0].reply.resolve({ watermark: "0", pending: null, retry: true }); await flush();
  assert.equal(h.calls.length, 2);
  h.calls[1].reply.resolve({ watermark: "0", pending: null, retry: true }); await flush();
  assert.equal(h.calls.length, 2);
  h.wake(); await flush();
  assert.equal(h.calls.length, 3);
  h.calls[2].reply.resolve({ watermark: "0", pending: null, retry: true }); await flush();
  assert.equal(h.calls.length, 4);
  h.owner.dispose();
});

test("a rejected current-day apply is never ACKed and a later valid intent can still apply", async () => {
  const h = harness(); h.rejectApply(); h.owner.setVisible(true); h.ready(); await flush();
  h.calls[0].reply.resolve(reply()); await flush();
  assert.equal(h.applied.length, 1);
  assert.equal(h.owner.commit(large), false);
  assert.deepEqual(h.calls.map((x) => x.request), [{ action: "peek" }]);
  h.acceptApply(); h.wake();
  const next = String(BigInt(large) + 1n);
  h.calls[1].reply.resolve(reply(next)); await flush();
  assert.equal(h.applied.length, 2);
  assert.equal(h.owner.commit(next), true);
  h.owner.dispose();
});

test("navigation parser rejects rounded sequences, invalid targets and unusable TTLs", () => {
  for (const value of [
    { watermark: 9007199254740993, pending: null },
    { watermark: "01", pending: null },
    { watermark: "18446744073709551616", pending: null },
    { watermark: "0", pending: undefined },
    { watermark: "0", pending: null, retry: "true" },
    reply(large, 0), reply(large, 10_001), reply(large, 1.5),
    { ...reply(), pending: { ...reply().pending!, destination: "settings" } },
    { ...reply(), watermark: "1" },
  ]) assert.throws(() => parsePanelNavigationReply(value));
  assert.equal(parsePanelNavigationReply(reply()).pending?.revision, large);
});

