import assert from "node:assert/strict";
import test from "node:test";
import type { Snapshot, UsageDashboardRequest } from "./contracts";
import { QueryLifecycle, queryScopeReady } from "./queryLifecycle";
import { changesQueryDomain, enablesQueryPrivacy } from "./queryDomain";
import {
  snapshotViewKey,
  snapshotViewReady,
  usageDashboardRequestKey,
  type SnapshotView,
} from "./snapshotView";

const now = new Date("2026-10-19T05:00:00Z");
const request: UsageDashboardRequest = { page: 0, pageSize: 20, quality: "all" };
const standard: SnapshotView = { enabled: false, includeTools: false, request };
const tools: SnapshotView = { ...standard, includeTools: true };
const overview: SnapshotView = { ...standard, enabled: true };
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}

test("canonical views distinguish all three projections and ignore ineffective request order or pagination", () => {
  assert.equal(new Set([standard, tools, overview].map(snapshotViewKey)).size, 3);
  const reordered: UsageDashboardRequest = { quality: "all", pageSize: 20, page: 0 };
  assert.equal(usageDashboardRequestKey(request), usageDashboardRequestKey(reordered));
  assert.equal(snapshotViewKey(overview), snapshotViewKey({ ...overview, request: reordered }));
  const otherPage = { ...request, page: 5, quality: "unpriced" as const };
  assert.equal(snapshotViewKey(standard), snapshotViewKey({ ...standard, request: otherPage }));
  assert.notEqual(snapshotViewKey(overview), snapshotViewKey({ ...overview, request: otherPage }));
});

test("sessions to tools rejects the delayed no-tools response before an effect invalidates its query ticket", async () => {
  const scope = new QueryLifecycle("", now);
  let view = standard;
  const old = scope.begin(now), key = snapshotViewKey(view);
  const response = deferred<Pick<Snapshot, "toolsLoaded">>();
  const accepted = response.promise.then((snapshot) =>
    scope.isCurrent(old, now) && snapshotViewReady(key, view, snapshot),
  );
  view = tools;
  assert.equal(scope.isCurrent(old, now), true);
  response.resolve({ toolsLoaded: false });
  assert.equal(await accepted, false);
  const fresh = scope.begin(now);
  assert.equal(scope.isCurrent(fresh, now), true);
  assert.equal(snapshotViewReady(snapshotViewKey(view), view, { toolsLoaded: true }), true);
});

test("tools to standard rejects late full details instead of replacing the newer projection", async () => {
  const scope = new QueryLifecycle("", now);
  let view = tools;
  const old = scope.begin(now), key = snapshotViewKey(view);
  const response = deferred<Pick<Snapshot, "toolsLoaded">>();
  const accepted = response.promise.then((snapshot) =>
    scope.isCurrent(old, now) && snapshotViewReady(key, view, snapshot),
  );
  view = standard;
  response.resolve({ toolsLoaded: true });
  assert.equal(await accepted, false);
  assert.equal(snapshotViewReady(snapshotViewKey(view), view, { toolsLoaded: false }), true);
});

test("dashboard page size, quality and page remain part of the response authority", () => {
  const key = snapshotViewKey(overview);
  for (const changed of [{ ...request, page: 1 }, { ...request, pageSize: 50 },
    { ...request, quality: "priced" as const }]) {
    const next = { ...overview, request: changed };
    assert.equal(snapshotViewReady(key, next, { toolsLoaded: false }), false);
    assert.equal(snapshotViewReady(snapshotViewKey(next), next, { toolsLoaded: false }), true);
  }
  assert.equal(snapshotViewReady(key, standard, { toolsLoaded: false }), false);
});

test("a matching view key cannot revive a ticket revoked by privacy or clear mutations", async () => {
  for (const reason of ["privacy", "clear_data"]) {
    const scope = new QueryLifecycle("", now);
    scope.update((q) => ({ ...q, sourceId: "selected-a", threadId: "thread-a" }), now);
    const filters = scope.state().query;
    const old = scope.begin(now), key = snapshotViewKey(tools);
    assert.equal(reason === "privacy" ? enablesQueryPrivacy({ privacyMode: true }) : changesQueryDomain(reason), true);
    scope.domainChanged();
    const write = deferred<void>();
    const saving = scope.withMutation(() => write.promise);
    assert.equal(snapshotViewReady(key, tools, { toolsLoaded: true }), true);
    assert.equal(scope.isCurrent(old, now), false);
    assert.equal(queryScopeReady(old, scope.state()), false);
    write.resolve(); await saving;
    assert.equal(scope.isCurrent(old, now), false);
    assert.deepEqual(scope.state().query, filters);
    assert.equal(scope.isCurrent(scope.begin(now), now), true);
  }
});

test("only legacy absence or true can satisfy full tools; false or unknown never means zero records", () => {
  const key = snapshotViewKey(tools);
  assert.equal(snapshotViewReady(key, tools, {}), true);
  assert.equal(snapshotViewReady(key, tools, { toolsLoaded: true }), true);
  assert.equal(snapshotViewReady(key, tools, { toolsLoaded: false }), false);
  for (const invalid of [null, "true", 0])
    assert.equal(snapshotViewReady(key, tools, { toolsLoaded: invalid } as unknown as Pick<Snapshot, "toolsLoaded">), false);
  assert.equal(snapshotViewReady(null, tools, { toolsLoaded: true }), false);
  assert.equal(snapshotViewReady(snapshotViewKey(standard), tools, {}), false);
});
