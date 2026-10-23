import test from "node:test";
import assert from "node:assert/strict";
import { createLocalScheduleClient, LocalScheduleReadModel, type LocalScheduleClient, type LocalScheduleViewState } from "./localScheduleClient";
import type { LocalSchedulesOverview, RecordLocalConsumption } from "./localScheduleContracts";

const overview = (revision = 1, scopeToken = "scope-a"): LocalSchedulesOverview => ({ schemaVersion: 1, revision, scopeToken, asOf: "2026-10-23T12:00:00Z", source: { id: "source-a", clientKind: "codex", enabled: true, configurationRevision: "1" }, templates: [], instances: [], reasons: [] });
const record: RecordLocalConsumption = { sourceId: "source-a", expectedScope: "scope-a", instanceId: "instance-a", idempotencyKey: "12345678-1234-4123-8123-123456789abc", occurredAt: "2026-10-23T12:00:00Z", amount: "2", note: "手动记录" };
const settle = () => new Promise(resolve => setTimeout(resolve, 0));
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(yes => resolve = yes); return { promise, resolve }; }
function fixture() {
  let wake: ((domain: boolean) => void) | null = null, value = overview(), removes = 0;
  const seen: LocalScheduleViewState[] = [], calls: string[] = [];
  const client: LocalScheduleClient = { read: async () => { calls.push("read"); return value; }, save: async () => { calls.push("save"); return value; }, status: async () => value, record: async () => value, void: async () => value,
    subscribe: async callback => { wake = callback; return () => { removes++; wake = null; }; } };
  const model = new LocalScheduleReadModel(client, "source-a", next => seen.push(next));
  return { model, client, seen, calls, wake: (domain = false) => wake?.(domain), value: (next: LocalSchedulesOverview) => value = next, removes: () => removes };
}
test("客户端保留 sourceId / expectedScope / 字符串量的准确 ABI，读写全局串行且不调用官方 RPC", async () => {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const client = createLocalScheduleClient(async<T>(command: string, args?: Record<string, unknown>) => { calls.push({ command, args }); return overview() as T; }, async () => () => {});
  await client.read("source-a"); await client.record(record);
  await client.void({ sourceId: "source-a", expectedScope: "scope-a", entryId: record.idempotencyKey, idempotencyKey: "12345678-1234-4123-8123-123456789abd", expectedRevision: 1 });
  assert.deepEqual(calls.map(c => c.command), ["read_local_schedules", "record_local_consumption", "void_local_consumption"]);
  assert.deepEqual(calls[0].args, { sourceId: "source-a" }); assert.deepEqual(calls[1].args, { payload: record });
  await assert.rejects(client.record({ ...record, expectedScope: "" }), /缺少/); assert.equal(calls.length, 3);
});
test("旧来源排队写入在真正 invoke 前取消，不会在切源后偷偷执行", async () => {
  const gate = deferred<LocalSchedulesOverview>(), calls: string[] = [];
  const client = createLocalScheduleClient(async<T>(command: string) => { calls.push(command); return (calls.length === 1 ? await gate.promise : overview()) as T; }, async () => () => {});
  const read = client.read("source-a"); let current = true; const write = client.record(record, () => current);
  await settle(); current = false; gate.resolve(overview()); await read; await assert.rejects(write, /范围已变化/);
  assert.deepEqual(calls, ["read_local_schedules"]);
});
test("事件风暴丢弃旧读结果，仅合并一次补读，卸载后订阅与迟到结果释放", async () => {
  const f = fixture(), gate = deferred<LocalSchedulesOverview>(); let reads = 0;
  f.client.read = async () => ++reads === 1 ? gate.promise : overview(9);
  f.model.start(); await settle(); for (let i = 0; i < 6; i++) f.wake(); gate.resolve(overview(1)); await settle(); await settle();
  assert.equal(reads, 2); assert.equal(f.model.state().overview!.revision, 9);
  assert.ok(!f.seen.some(s => s.overview?.revision === 1));
  f.model.dispose(); assert.equal(f.removes(), 1); f.wake(); await settle(); assert.equal(reads, 2);
});
test("写入互斥，clear 立即撤旧视图和表单，旧成功返回不能污染新 scope", async () => {
  const f = fixture(), gate = deferred<LocalSchedulesOverview>(); f.model.start(); await settle();
  let writes = 0; const operation = async () => { writes++; return gate.promise; };
  const first = f.model.mutate("record", "scope-a", operation, "保存成功");
  assert.equal(await f.model.mutate("record", "scope-a", operation, "重复"), false);
  f.wake(true); assert.equal(f.model.state().overview, null); assert.equal(f.model.state().formEpoch, 1);
  f.value(overview(0, "scope-after-clear")); gate.resolve(overview(2)); assert.equal(await first, false); await settle();
  assert.equal(writes, 1); assert.equal(f.model.state().overview!.scopeToken, "scope-after-clear");
  assert.ok(!f.seen.some(s => s.notice === "保存成功")); f.model.dispose();
});
test("新建写失败只重读而不重发写入；旧修订返回不能覆盖已接受视图", async () => {
  const f = fixture(); f.model.start(); await settle(); let writes = 0;
  const saved = await f.model.mutate("save", "scope-a", async () => { writes++; throw Error("回包失败，提交状态待核对"); }, "成功");
  await settle(); assert.equal(saved, false); assert.equal(writes, 1); assert.equal(f.calls.filter(c => c === "read").length, 2);
  assert.match(f.model.state().error!, /回包失败/);
  f.value(overview(10)); f.model.reload(); await settle(); f.value(overview(9)); f.model.reload(); await settle();
  assert.equal(f.model.state().overview!.revision, 10); assert.equal(f.model.state().fresh, false); f.model.dispose();
});
test("手动提交已落库但回包失败，同一意图重试沿用幂等键且只记一条消费", async () => {
  const entries = new Map<string, RecordLocalConsumption>(), keys: string[] = []; let first = true;
  const client = createLocalScheduleClient(async<T>(command: string, args?: Record<string, unknown>) => {
    assert.equal(command, "record_local_consumption"); const p = args!.payload as RecordLocalConsumption;
    keys.push(p.idempotencyKey); if (!entries.has(p.idempotencyKey)) entries.set(p.idempotencyKey, p);
    if (first) { first = false; throw Error("已提交但 IPC 回包断开"); }
    return overview(2) as T;
  }, async () => () => {});
  await assert.rejects(client.record(record), /回包/); await client.record(record);
  assert.deepEqual(keys, [record.idempotencyKey, record.idempotencyKey]); assert.equal(entries.size, 1);
  assert.equal([...entries.values()].reduce((n, p) => n + BigInt(p.amount), 0n), 2n);
});
test("异步订阅晚于卸载完成时立即退订，失活写回调不再发布", async () => {
  const f = fixture(), listenGate = deferred<() => void>(), readGate = deferred<LocalSchedulesOverview>(); let removed = 0;
  f.client.subscribe = async () => listenGate.promise; f.client.read = async () => readGate.promise;
  f.model.start(); const before = f.seen.length; f.model.dispose(); listenGate.resolve(() => removed++); readGate.resolve(overview(6));
  await settle(); assert.equal(removed, 1); assert.equal(f.seen.length, before);
});
