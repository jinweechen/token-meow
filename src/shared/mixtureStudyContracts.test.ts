import test from "node:test";
import assert from "node:assert/strict";
import { parseMixtureStudiesOverview, parseMixtureStudyConfig, type MixtureStudiesOverview, type MixtureStudy } from "./mixtureStudyContracts";
import { buildMixtureStudyInput, syntheticMixtureForm } from "./mixtureStudyForms";
const now=Date.parse("2026-10-23T12:00:00.700Z"),at=(offset=0)=>new Date(now+offset).toISOString();
const flags={quality:"conditional-manual" as const,constraintsVerified:false as const,notifyEligible:false as const,uncertaintyBounds:null};
const empty=():MixtureStudiesOverview=>({schemaVersion:1,revision:1,scopeToken:"scope-a",asOf:at(),source:{id:"source-a",clientKind:"codex",enabled:true,configurationRevision:"1"},studies:[],reasons:[]});
function study():MixtureStudy {return {id:"11111111-1111-4111-8111-111111111111",revision:1,sourceId:"source-a",configurationRevision:"1",bindingState:"ready",createdAt:at(),updatedAt:at(),config:buildMixtureStudyInput(syntheticMixtureForm(now),empty()).config,
  projection:{...flags,reasons:[],fit:{...flags,kind:"separated",weights:[{model:"合成模型 A",pointsPerMillionTokens:2},{model:"合成模型 B",pointsPerMillionTokens:5}],fixedMix:null,rank:2,conditionNumber:2,trainSamples:9,holdoutSamples:3,trainWape:0,holdoutWape:0,reasons:[]},rolling:{...flags,asOf:at(),endsAt:at(7200000),points:[{at:at(),usedPoints:4,remainingPoints:6,available:true},{at:at(900000),usedPoints:10,remainingPoints:0,available:false},{at:at(2700000),usedPoints:6,remainingPoints:4,available:true},{at:at(4500000),usedPoints:0,remainingPoints:10,available:true},{at:at(7200000),usedPoints:0,remainingPoints:10,available:true}],availabilityWindows:[{fromAt:at(),toAt:at(900000)},{fromAt:at(2700000),toAt:at(7200000)}],nextAvailableAt:at(),reasons:[]}}};}
function fixture(){return {...empty(),studies:[study()]};}
test("strict parser 保留手动旗标、独立模型关系与固定模拟边界，不引入官方资格",()=>{
  const value=parseMixtureStudiesOverview(fixture(),"source-a");assert.equal(value.studies[0].projection.fit.weights![1].pointsPerMillionTokens,5);
  assert.equal(value.studies[0].projection.notifyEligible,false);assert.equal(value.studies[0].projection.rolling.endsAt,at(7200000));
  assert.equal(value.studies[0].config.rolling.asOf,at());
});
test("任一层通知/约束/置信区间升级均拒绝",()=>{
  for(const change of [(v:any)=>v.studies[0].projection.notifyEligible=true,(v:any)=>v.studies[0].projection.fit.constraintsVerified=true,(v:any)=>v.studies[0].projection.rolling.uncertaintyBounds={lower:1,upper:2},(v:any)=>v.studies[0].projection.quality="validated"]){const v=fixture();change(v);assert.throws(()=>parseMixtureStudiesOverview(v));}
});
test("固定组合不给独立权重，留出失败不能残留可用系数，样本不足不可伪装 separated",()=>{
  const v=fixture(),f=v.studies[0].projection.fit;f.kind="fixed_mix";f.rank=1;f.weights=null;f.fixedMix={shares:[.5,.5],pointsPerMillionTokens:3.5};assert.ok(parseMixtureStudiesOverview(v));
  f.weights=[{model:"合成模型 A",pointsPerMillionTokens:2}];assert.throws(()=>parseMixtureStudiesOverview(v));
  const bad=fixture();bad.studies[0].projection.fit.holdoutWape=.21;assert.throws(()=>parseMixtureStudiesOverview(bad));
  bad.studies[0].projection.fit.kind="unavailable";assert.throws(()=>parseMixtureStudiesOverview(bad));
  bad.studies[0].projection.fit.weights=null;assert.ok(parseMixtureStudiesOverview(bad));
  const insufficient=fixture();insufficient.studies[0].projection.fit.trainSamples=3;assert.throws(()=>parseMixtureStudiesOverview(insufficient));
});
test("来源 ABA pending 仅两端未知可读，不能沿用旧权重或旧可用曲线",()=>{
  const v=fixture(),s=v.studies[0];v.source.configurationRevision="2";s.bindingState="pending_rebind";
  s.projection.fit={...s.projection.fit,kind:"unavailable",weights:null,rank:0,conditionNumber:null,trainSamples:0,holdoutSamples:0,trainWape:null,holdoutWape:null,reasons:["source_binding_changed"]};
  s.projection.rolling.points=[{at:at(),usedPoints:null,remainingPoints:null,available:null},{at:at(7200000),usedPoints:null,remainingPoints:null,available:null}];s.projection.rolling.availabilityWindows=[];s.projection.rolling.nextAvailableAt=null;
  assert.equal(parseMixtureStudiesOverview(v).studies[0].projection.rolling.points[0].usedPoints,null);
  s.projection.rolling.points=[];assert.throws(()=>parseMixtureStudiesOverview(v));
});
test("曲线端点、同刻唯一、有界可用区间及三字段同时未知强制校验",()=>{
  for(const change of [(v:any)=>v.studies[0].projection.rolling.points[0].remainingPoints=null,(v:any)=>v.studies[0].projection.rolling.points[1].at=at(),(v:any)=>v.studies[0].projection.rolling.endsAt=at(9000000),(v:any)=>v.studies[0].projection.rolling.availabilityWindows[0].toAt=null,(v:any)=>v.studies[0].projection.rolling.nextAvailableAt=at(7200001),(v:any)=>v.studies[0].projection.rolling.points.shift()]){const v=fixture();change(v);assert.throws(()=>parseMixtureStudiesOverview(v));}
});
test("输入限制空白 Token、跨列/跨记录身份、重叠样本、未来训练和未知模型标签",()=>{
  for(const change of [(v:any)=>v.samples[0].tokens[0]="",(v:any)=>v.samples[0].tokens=["1"],(v:any)=>v.samples[0].tokens=["0","0"],(v:any)=>v.samples[0].tokens[0]="9007199254740992",(v:any)=>v.rolling.charges[0].id=v.samples[0].id,(v:any)=>v.models[1]=v.models[0],(v:any)=>v.samples[0].toAt=v.samples[1].toAt,(v:any)=>v.samples.at(-1).toAt=at(1),(v:any)=>v.rolling.historyCompleteFrom=at(1),(v:any)=>v.rolling.charges[0].occurredAt=at(3600001),(v:any)=>v.condition.effort="x\u202ey"]){const v=study().config;change(v);assert.throws(()=>parseMixtureStudyConfig(v));}
});
test("跨来源、配置修订、重复 study、非规范时间和不精确计数拒绝",()=>{
  for(const change of [(v:any)=>v.studies[0].sourceId="other",(v:any)=>v.studies[0].configurationRevision="2",(v:any)=>v.studies.push(v.studies[0]),(v:any)=>v.asOf="2026-10-23T12:00:00Z",(v:any)=>v.source.configurationRevision=1,(v:any)=>v.revision=Number.MAX_SAFE_INTEGER+1]){const v=fixture();change(v);assert.throws(()=>parseMixtureStudiesOverview(v));}
  assert.throws(()=>parseMixtureStudiesOverview(fixture(),"other"));
});
