import assert from "node:assert/strict";
import test from "node:test";
import { demoSnapshot } from "./demo";
import {
  enablesQueryPrivacy,
  revokeQueryFacts,
} from "./queryDomain";
import { QueryLifecycle, queryScopeReady } from "./queryLifecycle";
import { ViewRefreshLoop } from "./viewRefreshLoop";

const now = new Date("2026-10-19T04:00:00Z");

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

test("a delayed nonprivate reply cannot refill after a privacy write starts or completes", async () => {
  const scope = new QueryLifecycle("", now);
  scope.update((query) => ({ ...query, sourceId: "selected-a" }), now);
  const filters = scope.state();
  const old = scope.begin(now);
  const reply = deferred<string>();
  const admitted = reply.promise.then((value) =>
    scope.isCurrent(old, now) ? value : null,
  );
  const write = deferred<void>();
  const saved = scope.withMutation(() => write.promise);
  assert.equal(scope.canRead(), false);
  assert.equal(scope.isCurrent(old, now), false);
  const during = scope.begin(now);
  assert.equal(scope.isCurrent(during, now), false);
  write.resolve();
  await saved;
  assert.equal(scope.canRead(), true);
  // Settlement may happen after the IPC returned, not just during the write.
  reply.resolve("old unredacted project and session names");
  assert.equal(await admitted, null);
  assert.equal(scope.isCurrent(during, now), false);
  assert.deepEqual(scope.state(), filters);
  const fresh = scope.begin(now);
  assert.equal(scope.isCurrent(fresh, now), true);
});

test("the visible scheduler skips reads during settings writes and restores the full fresh response", async () => {
  const scope = new QueryLifecycle("", now);
  const old = demoSnapshot();
  old.settings.privacyMode = false;
  let visible = old;
  const reads: ReturnType<typeof deferred<typeof old>>[] = [];
  const loop = new ViewRefreshLoop(async () => {
    if (!scope.canRead()) return;
    const ticket = scope.begin(now);
    const response = deferred<typeof old>();
    reads.push(response);
    const snapshot = await response.promise;
    if (scope.isCurrent(ticket, now)) visible = snapshot;
  }, { set: () => 1, clear: () => undefined });
  loop.setVisible(true);
  const oldRead = loop.request();
  const write = deferred<void>();
  const saved = scope.withMutation(() => write.promise);
  visible = revokeQueryFacts(visible, { privacyMode: true });
  const revoked = visible;
  void loop.request(true);
  reads[0].resolve(old);
  await oldRead;
  assert.equal(reads.length, 1);
  assert.equal(visible, revoked);
  assert.equal(visible.settings.privacyMode, true);
  assert.deepEqual(visible.sessions, []);
  write.resolve();
  await saved;
  const refreshed = loop.request(true);
  const complete = demoSnapshot();
  complete.settings.privacyMode = true;
  // The read result is retained whole, without client-side truncation.
  reads[1].resolve(complete);
  await refreshed;
  assert.equal(visible, complete);
  assert.equal(visible.sessions.length, complete.sessions.length);
  assert.equal(loop.diagnostics().running, false);
  loop.dispose();
});

test("failed settings writes revoke stale authority and allow authoritative recovery", async () => {
  const scope = new QueryLifecycle("", now);
  const old = scope.begin(now);
  const write = deferred<void>();
  const saved = scope.withMutation(() => write.promise);
  const during = scope.begin(now);
  const failure = new Error("settings did not commit");
  write.reject(failure);
  await assert.rejects(saved, (error) => error === failure);
  assert.equal(scope.canRead(), true);
  assert.equal(scope.isCurrent(old, now), false);
  assert.equal(scope.isCurrent(during, now), false);
  const recovery = scope.begin(now);
  assert.equal(scope.isCurrent(recovery, now), true);
  const recovered = demoSnapshot();
  recovered.settings.privacyMode = false;
  const protectedSnapshot = revokeQueryFacts(recovered, { privacyMode: true });
  assert.deepEqual(protectedSnapshot.projects, []);
  // Recovery applies the backend result, not an optimistic settings rollback.
  const visible = scope.isCurrent(recovery, now) ? recovered : protectedSnapshot;
  assert.equal(visible, recovered);
  assert.equal(visible.settings.privacyMode, false);
  assert.ok(visible.projects.length > 0);
});

test("a synchronous settings failure also releases the read gate", async () => {
  const scope = new QueryLifecycle("", now);
  const failure = new Error("invoke failed before creating its promise");
  await assert.rejects(scope.withMutation(() => { throw failure; }),
    (error) => error === failure);
  assert.equal(scope.canRead(), true);
  assert.equal(scope.isCurrent(scope.begin(now), now), true);
});

test("overlapping domain and settings writes keep the gate closed until both settle", async () => {
  const scope = new QueryLifecycle("", now);
  const domain = deferred<void>();
  const settings = deferred<void>();
  const domainWrite = scope.withMutation(() => domain.promise);
  const settingsWrite = scope.withMutation(() => settings.promise);
  domain.resolve();
  await domainWrite;
  assert.equal(scope.canRead(), false);
  const between = scope.begin(now);
  assert.equal(scope.isCurrent(between, now), false);
  settings.reject(new Error("second write failed"));
  await assert.rejects(settingsWrite);
  assert.equal(scope.canRead(), true);
  assert.equal(scope.isCurrent(between, now), false);
  assert.equal(scope.isCurrent(scope.begin(now), now), true);
});

test("cross-window settings invalidation rejects delayed success and error authority without blocking new reads", async () => {
  for (const outcome of ["success", "failure"] as const) {
    const scope = new QueryLifecycle("", now);
    const old = scope.begin(now);
    const response = deferred<string>();
    const settled = response.promise.then(
      (value) => scope.isCurrent(old, now) ? value : null,
      (error) => scope.isCurrent(old, now) ? error : null,
    );
    // pet-options-changed uses the existing ticket invalidation before refresh.
    scope.invalidate();
    assert.equal(scope.canRead(), true);
    if (outcome === "success") response.resolve("pre-change private mode off");
    else response.reject(new Error("obsolete read failure"));
    assert.equal(await settled, null);
    assert.equal(scope.isCurrent(scope.begin(now), now), true);
  }
});

test("enabling privacy revokes every accepted fact collection and its query authority immediately", () => {
  const scope = new QueryLifecycle("", now);
  const accepted = scope.begin(now);
  const old = demoSnapshot();
  old.settings = { ...old.settings, privacyMode: false, petScale: 1.5 };
  const before = structuredClone(old);
  const payload = { privacyMode: true, unrelated: "not projected" };
  assert.equal(enablesQueryPrivacy(payload), true);
  scope.domainChanged();
  const privatePending = revokeQueryFacts(old, { privacyMode: true });
  assert.equal(queryScopeReady(accepted, scope.state()), false);
  assert.equal(scope.isCurrent(accepted, now), false);
  assert.equal(privatePending.settings.privacyMode, true);
  assert.equal(privatePending.settings.petScale, 1.5);
  for (const key of ["projects", "sessions", "turns", "tools", "inbox", "models", "daily", "budgets"] as const)
    assert.deepEqual(privatePending[key], [], key);
  assert.equal(privatePending.usageDashboard, undefined);
  assert.equal(privatePending.sources?.length ?? 0, 0);
  assert.deepEqual(old, before);
});

test("other settings and malformed privacy payloads do not request destructive fact revocation", () => {
  for (const payload of [null, undefined, true, {}, { privacyMode: false },
    { privacyMode: "true" }, { privacyMode: 1 }, { petScale: 2 }])
    assert.equal(enablesQueryPrivacy(payload), false);
});
