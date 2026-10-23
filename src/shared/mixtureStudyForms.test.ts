import test from "node:test";
import assert from "node:assert/strict";
import { buildMixtureStudyInput, changeMixtureModels, newMixtureStudyForm, syntheticMixtureForm } from "./mixtureStudyForms";
import type { MixtureStudiesOverview } from "./mixtureStudyContracts";
const now=Date.parse("2026-10-23T12:00:00.700Z");
const overview=():MixtureStudiesOverview=>({schemaVersion:1,revision:1,scopeToken:"scope-a",asOf:new Date(now).toISOString(),source:{id:"source-a",clientKind:"gemini",enabled:true,configurationRevision:"1"},studies:[],reasons:[]});
test("新表单不预装账号或可用样本，示例仅显式填表并以手动合成标签保存",()=>{
  const f=newMixtureStudyForm(now);assert.equal(f.samples.length,0);assert.equal(f.charges.length,0);assert.equal(f.condition.accountLabel,"");assert.equal(f.historyCompleteFrom,"");
  assert.throws(()=>buildMixtureStudyInput(f,overview()));
  const input=buildMixtureStudyInput(syntheticMixtureForm(now),overview());assert.match(input.config.title,/合成示例/);assert.match(input.config.condition.accountLabel,/非官方/);assert.equal(input.config.samples.length,12);
  assert.equal(input.config.rolling.charges[1].kind,"models");assert.equal(input.expectedScope,"scope-a");assert.equal(input.sourceId,"source-a");assert.equal(input.expectedRevision,null);
});
test("时刻规范为UTC毫秒且基准冻结，不以当前墙钟推进模型输入",()=>{
  const f=syntheticMixtureForm(now),first=buildMixtureStudyInput(f,overview()),second=buildMixtureStudyInput(f,overview());
  assert.deepEqual(second,first);assert.equal(first.config.rolling.asOf,"2026-10-23T12:00:00.700Z");
  f.historyCompleteFrom="";assert.equal(buildMixtureStudyInput(f,overview()).config.rolling.historyCompleteFrom,null);
  f.asOf="2026-02-30T20:00";assert.throws(()=>buildMixtureStudyInput(f,overview()));
});
test("模型列增加为空，删除只删除对应列，不把旧Token错配到其它模型",()=>{
  const f=syntheticMixtureForm(now),before=[...f.samples[1].tokens],added=changeMixtureModels(f,2,"模型C");
  assert.deepEqual(added.samples[1].tokens,[...before,""]);assert.throws(()=>buildMixtureStudyInput(added,overview()));
  const removed=changeMixtureModels(f,0,null);assert.deepEqual(removed.models,[f.models[1]]);assert.deepEqual(removed.samples[1].tokens,[f.samples[1].tokens[1]]);assert.deepEqual(f.samples[1].tokens,before);
});
test("点数和窗口有明确上限，不接受指数、空值、负数或静默截断",()=>{
  for(const patch of [{capacityPoints:""},{requiredPoints:"0"},{capacityPoints:"1e3"},{capacityPoints:"1000000001"},{windowMinutes:"10081"},{windowMinutes:"1.5"},{requiredPoints:"11"}])assert.throws(()=>buildMixtureStudyInput({...syntheticMixtureForm(now),...patch},overview()));
  const f=syntheticMixtureForm(now);f.samples[0].deltaPoints="0";assert.equal(buildMixtureStudyInput(f,overview()).config.samples[0].deltaPoints,0);
  f.samples[0].tokens[1]="";assert.throws(()=>buildMixtureStudyInput(f,overview()));
});
test("编辑以捕获scope和revision保存；不能把别的来源实验搬到当前来源",()=>{
  const base=buildMixtureStudyInput(syntheticMixtureForm(now),overview());
  const s={id:"11111111-1111-4111-8111-111111111111",sourceId:"source-a",revision:9} as any;
  const value=buildMixtureStudyInput(syntheticMixtureForm(now),overview(),s);assert.equal(value.id,s.id);assert.equal(value.expectedRevision,9);assert.equal(value.expectedScope,base.expectedScope);
  assert.throws(()=>buildMixtureStudyInput(syntheticMixtureForm(now),overview(),{...s,sourceId:"other"}));
});
