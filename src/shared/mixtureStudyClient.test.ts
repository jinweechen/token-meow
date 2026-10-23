import test from "node:test";
import assert from "node:assert/strict";
import { createMixtureStudyClient, MixtureStudyReadModel, type MixtureStudyClient, type MixtureStudyViewState } from "./mixtureStudyClient";
import type { MixtureStudiesOverview } from "./mixtureStudyContracts";
import { buildMixtureStudyInput, syntheticMixtureForm } from "./mixtureStudyForms";
const now=Date.parse("2026-10-23T12:00:00.700Z");
const overview=(revision=1,scopeToken="scope-a"):MixtureStudiesOverview=>({schemaVersion:1,revision,scopeToken,asOf:new Date(now).toISOString(),source:{id:"source-a",clientKind:"codex",enabled:true,configurationRevision:"1"},studies:[],reasons:[]});
const settle=()=>new Promise(resolve=>setTimeout(resolve,0));
function deferred<T>(){let resolve!:(value:T)=>void;const promise=new Promise<T>(yes=>resolve=yes);return{promise,resolve};}
function fixture(){let wake:((domain:boolean)=>void)|null=null,value=overview(),removed=0;const seen:MixtureStudyViewState[]=[];const calls:string[]=[];
  const client:MixtureStudyClient={read:async()=>{calls.push("read");return value;},save:async()=>value,delete:async()=>value,subscribe:async cb=>{wake=cb;return()=>{removed++;wake=null;};}};
  const model=new MixtureStudyReadModel(client,"source-a",v=>seen.push(v));return{model,client,seen,calls,wake:(domain=false)=>wake?.(domain),value:(v:MixtureStudiesOverview)=>value=v,removed:()=>removed};}
test("客户端准确传scope/revision/config，只有本地read/save/delete，不发官方RPC",async()=>{
  const calls:{command:string;args?:Record<string,unknown>}[]=[];
  const c=createMixtureStudyClient(async<T>(command:string,args?:Record<string,unknown>)=>{calls.push({command,args});return overview() as T;},async()=>()=>{}),input=buildMixtureStudyInput(syntheticMixtureForm(now),overview());
  await c.read("source-a");await c.save(input);await c.delete({sourceId:"source-a",expectedScope:"scope-a",id:"11111111-1111-4111-8111-111111111111",expectedRevision:1});
  assert.deepEqual(calls.map(v=>v.command),["read_mixture_studies","save_mixture_study","delete_mixture_study"]);assert.deepEqual(calls[1].args,{payload:input});
  await assert.rejects(c.save({...input,expectedScope:""}),/缺少/);assert.equal(calls.length,3);
});
test("排队写入真正invoke前验证owner，切源后不执行旧保存",async()=>{
  const gate=deferred<MixtureStudiesOverview>(),calls:string[]=[];
  const c=createMixtureStudyClient(async<T>(command:string)=>{calls.push(command);return(calls.length===1?await gate.promise:overview()) as T;},async()=>()=>{});
  const first=c.read("source-a");let current=true;const second=c.save(buildMixtureStudyInput(syntheticMixtureForm(now),overview()),()=>current);await settle();current=false;gate.resolve(overview());await first;await assert.rejects(second,/范围已变化/);assert.deepEqual(calls,["read_mixture_studies"]);
});
test("更新风暴合并一次补读且旧结果不覆盖当前版本",async()=>{
  const f=fixture(),gate=deferred<MixtureStudiesOverview>();let reads=0;f.client.read=async()=>++reads===1?gate.promise:overview(9);f.model.start();await settle();for(let i=0;i<8;i++)f.wake();gate.resolve(overview(1));await settle();await settle();
  assert.equal(reads,2);assert.equal(f.model.state().overview!.revision,9);assert.ok(!f.seen.some(v=>v.overview?.revision===1));f.model.dispose();assert.equal(f.removed(),1);
});
test("clear立即关闭旧scope并撤结果，迟到成功不得复活旧手动实验",async()=>{
  const f=fixture(),gate=deferred<MixtureStudiesOverview>();f.model.start();await settle();let writes=0;
  const op=async()=>{writes++;return gate.promise;};const first=f.model.mutate("save","scope-a",op,"成功");assert.equal(await f.model.mutate("save","scope-a",op,"重复"),false);
  f.wake(true);assert.equal(f.model.state().overview,null);assert.equal(f.model.state().formEpoch,1);f.value(overview(0,"clear-scope"));gate.resolve(overview(2));assert.equal(await first,false);await settle();assert.equal(writes,1);assert.equal(f.model.state().overview!.scopeToken,"clear-scope");assert.ok(!f.seen.some(v=>v.notice==="成功"));f.model.dispose();
});
test("新建回包失败只重读不重发，错误保留可见，读取不静默改冻结配置",async()=>{
  const f=fixture();f.model.start();await settle();let writes=0;
  assert.equal(await f.model.mutate("save","scope-a",async()=>{writes++;throw Error("已提交但回包丢失");},"成功"),false);await settle();assert.equal(writes,1);assert.equal(f.calls.length,2);assert.match(f.model.state().error!,/回包丢失/);
  f.value(overview(10));f.model.reload();await settle();f.value(overview(9));f.model.reload();await settle();assert.equal(f.model.state().overview!.revision,10);assert.equal(f.model.state().fresh,false);f.model.dispose();
});
test("监听晚于卸载建立时立即退订，旧读不会再次发布",async()=>{
  const f=fixture(),listen=deferred<()=>void>(),read=deferred<MixtureStudiesOverview>();let removed=0;f.client.subscribe=async()=>listen.promise;f.client.read=async()=>read.promise;f.model.start();const before=f.seen.length;f.model.dispose();listen.resolve(()=>removed++);read.resolve(overview(9));await settle();assert.equal(removed,1);assert.equal(f.seen.length,before);
});
