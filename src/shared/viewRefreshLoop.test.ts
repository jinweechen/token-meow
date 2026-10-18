import assert from "node:assert/strict";
import test from "node:test";
import { ViewRefreshLoop, type IntervalHost } from "./viewRefreshLoop";
test("production visible view scheduler owns one interval, stops hidden queries and refreshes once on recovery", async () => {
  let serial = 0,
    calls = 0;
  const timers = new Map<number, () => void>();
  const host: IntervalHost = {
    set: (callback) => {
      const id = ++serial;
      timers.set(id, callback);
      return id;
    },
    clear: (id) => {
      timers.delete(id);
    },
  };
  const loop = new ViewRefreshLoop(async () => {
    calls++;
  }, host);
  loop.setVisible(true);
  await Promise.resolve();
  assert.equal(calls, 1);
  assert.equal(timers.size, 1);
  loop.setVisible(true);
  assert.equal(timers.size, 1);
  assert.equal(calls, 1);
  loop.setVisible(false);
  assert.equal(timers.size, 0);
  await loop.request();
  assert.equal(calls, 1);
  for (let index = 0; index < 30; index++) {
    loop.setVisible(true);
    await Promise.resolve();
    assert.equal(timers.size, 1);
    loop.setVisible(false);
    assert.equal(timers.size, 0);
  }
  assert.equal(calls, 31);
  loop.dispose();
  await loop.request();
  assert.equal(calls, 31);
  assert.equal(timers.size, 0);
});
test("production scheduler does not launch overlapping full snapshot requests", async () => {
  let calls = 0,
    complete: (() => void) | undefined;
  const loop = new ViewRefreshLoop(
    () => {
      calls++;
      return new Promise<void>((resolve) => {
        complete = resolve;
      });
    },
    { set: () => 1, clear: () => undefined },
  );
  loop.setVisible(true);
  const shared = loop.request();
  assert.equal(calls, 1);
  loop.setVisible(false);
  complete?.();
  await shared;
  loop.dispose();
});
test("resume during an in-flight query schedules exactly one current-state refresh", async () => {
  let calls = 0;
  const pending: (() => void)[] = [];
  const loop = new ViewRefreshLoop(
    () => {
      calls++;
      return new Promise<void>((resolve) => pending.push(resolve));
    },
    { set: () => 1, clear: () => undefined },
  );
  loop.setVisible(true);
  loop.setVisible(false);
  loop.setVisible(true);
  loop.setVisible(true);
  assert.equal(calls, 1);
  pending.shift()!();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(calls, 2);
  pending.shift()!();
  await Promise.resolve();
  loop.dispose();
  assert.equal(calls, 2);
});

test("mutation and event refreshes share one flight and await a single current-state follow-up", async () => {
  let calls = 0,
    active = 0,
    maximumActive = 0,
    currentScope = "before";
  const seenScopes: string[] = [];
  const pending: (() => void)[] = [];
  const loop = new ViewRefreshLoop(
    async () => {
      calls++;
      active++;
      maximumActive = Math.max(maximumActive, active);
      seenScopes.push(currentScope);
      await new Promise<void>((resolve) => pending.push(resolve));
      active--;
    },
    { set: () => 1, clear: () => undefined },
  );
  loop.setVisible(true);
  const first = loop.request();
  currentScope = "after";
  const mutation = loop.request(true);
  let completed = false;
  void mutation.then(() => { completed = true; });
  for (let index = 0; index < 10; index++) {
    assert.equal(loop.request(true), first);
    assert.equal(loop.request(), first);
  }
  assert.equal(mutation, first);
  assert.equal(calls, 1);
  assert.equal(completed, false);
  pending.shift()!();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(calls, 2);
  assert.equal(completed, false);
  assert.deepEqual(seenScopes, ["before", "after"]);
  pending.shift()!();
  await mutation;
  assert.equal(completed, true);
  assert.equal(maximumActive, 1);
  assert.equal(calls, 2);
  loop.dispose();
});

test("hidden and disposed views discard queued reads without reopening queries", async () => {
  for (const operation of ["hide", "dispose"] as const) {
    let calls = 0;
    let complete!: () => void;
    const loop = new ViewRefreshLoop(
      () => {
        calls++;
        return new Promise<void>((resolve) => { complete = resolve; });
      },
      { set: () => 1, clear: () => undefined },
    );
    loop.setVisible(true);
    const pending = loop.request(true);
    if (operation === "hide") loop.setVisible(false);
    else loop.dispose();
    complete();
    await pending;
    assert.equal(calls, 1);
    assert.equal(loop.diagnostics().running, false);
    await loop.request(true);
    assert.equal(calls, 1);
    loop.dispose();
  }
});

test("rejected reads are reported once and queued or later reads can recover", async () => {
  const errors: unknown[] = [];
  let calls = 0;
  let reject!: (error: unknown) => void;
  const failure = new Error("snapshot read failed");
  const loop = new ViewRefreshLoop(
    () => {
      calls++;
      return calls === 1
        ? new Promise<void>((_resolve, rejectRead) => { reject = rejectRead; })
        : Promise.resolve();
    },
    { set: () => 1, clear: () => undefined },
    (error) => { errors.push(error); },
  );
  loop.setVisible(true);
  const recovery = loop.request(true);
  reject(failure);
  await recovery;
  assert.deepEqual(errors, [failure]);
  assert.equal(calls, 2);
  assert.equal(loop.diagnostics().running, false);
  await loop.request();
  assert.equal(calls, 3);
  loop.dispose();
});

test("a synchronous read failure releases flight ownership for the next request", async () => {
  let calls = 0;
  const errors: unknown[] = [];
  const failure = new Error("synchronous snapshot failure");
  const loop = new ViewRefreshLoop(
    () => {
      calls++;
      if (calls === 1) throw failure;
      return Promise.resolve();
    },
    { set: () => 1, clear: () => undefined },
    (error) => { errors.push(error); },
  );
  loop.setVisible(true);
  await loop.request();
  assert.deepEqual(errors, [failure]);
  assert.equal(calls, 2);
  assert.equal(loop.diagnostics().running, false);
  loop.dispose();
});
