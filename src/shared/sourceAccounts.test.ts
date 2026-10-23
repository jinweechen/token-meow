import test from "node:test";
import assert from "node:assert/strict";
import { createSourceAccountsClient, parseSourceAccountsOverview, sourceAccountStatus, SourceAccountsReadModel,
  type SourceAccountRow, type SourceAccountsOverview, type SourceAccountsClient, type SourceAccountsView } from "./sourceAccounts";
import type { SourceInstance } from "./contracts";
const at = Date.parse("2026-10-23T12:00:00Z");
const row = (id = "source-a", percent: number | null = 12): SourceAccountRow => ({ sourceId: id, clientKind: "codex", primary: id === "source-a", enabled: true,
  bindingGeneration: "1", status: "ready", identityStatus: "unproven", snapshot: { sourceId: id, status: "connected", collectedAt: new Date(at).toISOString(), buckets: [
    { id: "quota", name: "独立窗口", primary: { usedPercent: percent, windowDurationMins: 300, resetsAt: (at + 60000) / 1000 } }] }, lastAttemptAt: new Date(at).toISOString(), reasons: [] });
const overview = (rows = [row()]): SourceAccountsOverview => ({ asOf: new Date(at).toISOString(), sourceCount: rows.length, confirmedAccountCount: null, rows });
const sources: SourceInstance[] = [{ id: "source-a", root: "D:/explicit-a", clientKind: "codex", enabled: true }, { id: "source-b", root: "D:/explicit-b", clientKind: "codex", enabled: true }, { id: "source-c", root: "D:/explicit-c", clientKind: "claude", enabled: true }];
const settle = () => new Promise(resolve => setTimeout(resolve, 0));
function deferred<T>() { let resolve!: (v: T) => void, reject!: (v: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function fixture(initial = overview()) {
  let wake: (() => void) | undefined;
  let value = initial;
  const seen: SourceAccountsView[] = [], calls: string[] = [];
  const client: SourceAccountsClient = { read: async () => { calls.push("read"); return value; }, refresh: async id => { calls.push(`refresh:${id}`); return value; }, subscribe: async fn => { wake = fn; return () => { wake = undefined; }; } };
  const model = new SourceAccountsReadModel(client, sources, state => seen.push(state));
  return { model, client, calls, seen, wake: () => wake?.(), set: (next: SourceAccountsOverview) => value = next };
}
test("parser keeps source rows and unknown account count; percentages remain separate and zero is valid", () => {
  const parsed = parseSourceAccountsOverview(overview([row("source-a", 0), row("source-b", 38)]));
  assert.equal(parsed.confirmedAccountCount, null);
  assert.deepEqual(parsed.rows.map(r => r.snapshot!.buckets[0].primary!.usedPercent), [0, 38]);
  assert.equal(parsed.sourceCount, 2);
});
test("unknown numeric fields remain null rather than zero and irrelevant identity fields are not projected", () => {
  const input = structuredClone(overview()) as any;
  input.rows[0].snapshot.accountScope = "PRIVATE_ACCOUNT_ID";
  input.rows[0].snapshot.buckets[0].primary = { usedPercent: -1, windowDurationMins: 0, resetsAt: "not-a-time" };
  const value = parseSourceAccountsOverview(input);
  assert.deepEqual(value.rows[0].snapshot!.buckets[0].primary, { usedPercent: null, windowDurationMins: null, resetsAt: null });
  assert.ok(!JSON.stringify(value).includes("PRIVATE_ACCOUNT_ID"));
});
test("parser rejects fabricated account counts, duplicate sources, wrong snapshot source and imprecise generation", () => {
  for (const mutate of [
    (v:any)=>v.confirmedAccountCount=0,
    (v:any)=>v.sourceCount=2,
    (v:any)=>{v.rows.push(v.rows[0]);v.sourceCount=2;},
    (v:any)=>v.rows[0].snapshot.sourceId="other",
    (v:any)=>v.rows[0].bindingGeneration="01",
    (v:any)=>v.rows[0].bindingGeneration="18446744073709551616",
    (v:any)=>v.rows[0].identityStatus="verified",
  ]) { const v=structuredClone(overview());mutate(v);assert.throws(()=>parseSourceAccountsOverview(v)); }
  const valid = overview();valid.rows[0].bindingGeneration="18446744073709551615";assert.ok(parseSourceAccountsOverview(valid));
});
test("stale snapshot wins over a ready row, missing time stays stale and unsupported logs never become official quota", () => {
  assert.equal(sourceAccountStatus(row(),at),"ready");
  const stale=row();stale.snapshot!.status="stale";assert.equal(sourceAccountStatus(stale,at),"stale");
  stale.snapshot!.status="connected";stale.snapshot!.collectedAt=null;assert.equal(sourceAccountStatus(stale,at),"stale");
  assert.equal(sourceAccountStatus({...row(),clientKind:"claude"},at),"unsupported");
  assert.equal(sourceAccountStatus({...row(),enabled:false},at),"disabled");
  assert.equal(sourceAccountStatus({...row(),status:"unavailable"},at),"stale");
  assert.equal(sourceAccountStatus(row(),at+60000),"stale");
});
test("client serializes native calls, read is cache-only and an obsolete queued refresh never invokes RPC", async () => {
  const gate=deferred<unknown>(),calls:{command:string;args?:Record<string,unknown>}[]=[];
  const client=createSourceAccountsClient(async<T>(command:string,args?:Record<string,unknown>)=>{calls.push({command,args});return (calls.length===1?await gate.promise:overview()) as T;},async()=>()=>{});
  const first=client.read();let current=true;const second=client.refresh("source-b",()=>current);current=false;
  await settle();assert.deepEqual(calls.map(c=>c.command),["read_source_accounts"]);
  gate.resolve(overview());await first;await assert.rejects(second,/范围已变化/);
  await client.refresh("source-b");assert.deepEqual(calls[1],{command:"refresh_source_account",args:{sourceId:"source-b"}});
});
test("event storms discard the old reply and coalesce into one follow-up cache read",async()=>{
  const f=fixture(),gate=deferred<SourceAccountsOverview>();let reads=0;
  f.client.read=async()=>{reads++;return reads===1?gate.promise:overview([row("source-b",38)]);};
  f.model.start();await settle();f.wake();f.wake();f.wake();assert.equal(reads,1);
  gate.resolve(overview([row("source-a",99)]));await settle();await settle();
  assert.equal(reads,2);assert.equal(f.model.state().overview!.rows[0].sourceId,"source-b");
  assert.ok(!f.seen.some(s=>s.overview?.rows.some(r=>r.snapshot?.buckets[0].primary?.usedPercent===99)));f.model.dispose();
});
test("one refresh at a time; unsupported logs cannot invoke account refresh",async()=>{
  const c={...row("source-c"),clientKind:"claude",status:"unsupported" as const,snapshot:null};
  const f=fixture(overview([row(),row("source-b"),c]));f.model.start();await settle();
  assert.equal(await f.model.refresh("source-c"),false);
  const gate=deferred<SourceAccountsOverview>();f.client.refresh=async id=>{f.calls.push(`refresh:${id}`);return gate.promise;};
  const first=f.model.refresh("source-b");assert.equal(await f.model.refresh("source-a"),false);assert.equal(f.model.state().refreshingSourceId,"source-b");
  gate.resolve(overview([row(),row("source-b",40),c]));assert.equal(await first,true);assert.equal(f.calls.filter(c=>c.startsWith("refresh:")).length,1);f.model.dispose();
});
test("failed row preserves old timestamp and stale values without degrading another source; a cache reread cannot renew them",async()=>{
  const f=fixture(overview([row(),row("source-b",38)]));f.model.start();await settle();
  f.client.refresh=async()=>{throw Error("synthetic refusal");};assert.equal(await f.model.refresh("source-b"),false);
  const state=f.model.state();assert.equal(state.overview!.rows[0].status,"ready");assert.equal(state.overview!.rows[1].status,"stale");
  assert.equal(state.overview!.rows[1].snapshot!.collectedAt,new Date(at).toISOString());assert.equal(state.rowErrors["source-b"],"synthetic refusal");
  f.wake();await settle();assert.equal(f.model.state().overview!.rows[1].status,"stale");
  const fresh=row("source-b",39);fresh.snapshot!.collectedAt=new Date(at+1000).toISOString();f.set(overview([row(),fresh]));f.wake();await settle();
  assert.equal(f.model.state().overview!.rows[1].status,"ready");assert.equal(f.model.state().rowErrors["source-b"],undefined);f.model.dispose();
});
test("registered source configuration and monotone binding generation fence old rows",async()=>{
  const initial=row();initial.bindingGeneration="2";const f=fixture(overview([initial,{...row("source-c"),clientKind:"codex"}]));
  f.model.start();await settle();assert.deepEqual(f.model.state().overview!.rows.map(r=>r.sourceId),["source-a"]);
  f.set(overview());f.wake();await settle();assert.equal(f.model.state().overview,null);assert.match(f.model.state().error!,/绑定快照已过时/);f.model.dispose();
});
test("disposal removes late subscriptions and no late read can publish",async()=>{
  const f=fixture(),gate=deferred<SourceAccountsOverview>(),subscription=deferred<()=>void>();let removed=0;
  f.client.read=()=>gate.promise;f.client.subscribe=()=>subscription.promise;f.model.start();const before=f.seen.length;f.model.dispose();
  subscription.resolve(()=>removed++);gate.resolve(overview());await settle();assert.equal(removed,1);assert.equal(f.seen.length,before);
});
