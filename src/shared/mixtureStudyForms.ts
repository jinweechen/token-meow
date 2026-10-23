import type { MixtureCharge, MixtureStudiesOverview, MixtureStudy, MixtureStudyConfig, SaveMixtureStudy } from "./mixtureStudyContracts";
import { parseMixtureStudyConfig } from "./mixtureStudyContracts";
import { localScheduleDateInput, localScheduleDateValue } from "./localScheduleForms";

export interface MixtureSampleForm { id:string; fromAt:string; toAt:string; deltaPoints:string; tokens:string[] }
export interface MixtureChargeForm { id:string; occurredAt:string; kind:"points"|"models"; amountPoints:string; tokens:string[] }
export interface MixtureStudyForm {
  title:string; condition:MixtureStudyConfig["condition"]; models:string[]; samples:MixtureSampleForm[];
  asOf:string; historyCompleteFrom:string; capacityPoints:string; requiredPoints:string; windowMinutes:string; charges:MixtureChargeForm[]; synthetic:boolean;
}
const inputTime=(time:string)=>localScheduleDateInput(time,"milliseconds");
function time(value:string,label:string) {
  const date=localScheduleDateValue(value);
  if (!date) throw Error(`${label}须为有效上海时间，保留原毫秒边界。`);
  const result=new Date(date).toISOString();
  if (result.startsWith("0000")||result.length!==24) throw Error("日期年份须为 1～9999。");
  return result;
}
function point(value:string,label:string) {
  const v=value.trim();
  if (v.length>48||!/^(?:0|[1-9]\d*)(?:\.\d+)?$/.test(v)||!Number.isFinite(Number(v))) throw Error(`${label}须为明确的非负十进制数，空白不是零。`);
  return Number(v);
}
export function newMixtureStudyForm(now=Date.now()):MixtureStudyForm {
  return {title:"",condition:{accountLabel:"",windowLabel:"",effort:"",speed:""},models:["",""],samples:[],
    asOf:inputTime(new Date(now).toISOString()),historyCompleteFrom:"",capacityPoints:"",requiredPoints:"",windowMinutes:"60",charges:[],synthetic:false};
}
export function mixtureFormFromStudy(study:MixtureStudy):MixtureStudyForm {
  const c=study.config;
  return {title:c.title,condition:{...c.condition},models:[...c.models],samples:c.samples.map(s=>({...s,fromAt:inputTime(s.fromAt),toAt:inputTime(s.toAt),deltaPoints:String(s.deltaPoints),tokens:[...s.tokens]})),
    asOf:inputTime(c.rolling.asOf),historyCompleteFrom:c.rolling.historyCompleteFrom?inputTime(c.rolling.historyCompleteFrom):"",
    capacityPoints:String(c.rolling.capacityPoints),requiredPoints:String(c.rolling.requiredPoints),windowMinutes:String(c.rolling.windowMinutes),
    charges:c.rolling.charges.map(s=>({...s,occurredAt:inputTime(s.occurredAt),amountPoints:s.kind==="points"?String(s.amountPoints):"",tokens:s.kind==="models"?[...s.tokens]:c.models.map(()=>"")})),synthetic:false};
}
export function buildMixtureStudyInput(form:MixtureStudyForm,overview:MixtureStudiesOverview,existing?:MixtureStudy|null):SaveMixtureStudy {
  if (!overview.source.enabled||!overview.scopeToken||existing&&existing.sourceId!==overview.source.id) throw Error("请先读取当前已启用来源的实验范围。");
  if (!/^[1-9]\d{0,4}$/.test(form.windowMinutes)) throw Error("窗口须为 1～10080 的整数分钟。");
  const samples=form.samples.map((s,i)=>({id:s.id,fromAt:time(s.fromAt,`样本 ${i+1} 开始`),toAt:time(s.toAt,`样本 ${i+1} 结束`),deltaPoints:point(s.deltaPoints,`样本 ${i+1} 手动增量`),tokens:s.tokens.map(v=>v.trim())})).sort((a,b)=>Date.parse(a.fromAt)-Date.parse(b.fromAt));
  const charges:MixtureCharge[]=form.charges.map((c,i)=>c.kind==="points"?{id:c.id,occurredAt:time(c.occurredAt,`进入记录 ${i+1}`),kind:"points",amountPoints:point(c.amountPoints,`进入记录 ${i+1} 点数`)}
    :{id:c.id,occurredAt:time(c.occurredAt,`进入记录 ${i+1}`),kind:"models",tokens:c.tokens.map(v=>v.trim())});
  const config=parseMixtureStudyConfig({title:form.title.trim(),condition:Object.fromEntries(Object.entries(form.condition).map(([k,v])=>[k,v.trim()])),models:form.models.map(v=>v.trim()),samples,
    rolling:{asOf:time(form.asOf,"冻结基准"),historyCompleteFrom:form.historyCompleteFrom?time(form.historyCompleteFrom,"历史覆盖起点"):null,
      capacityPoints:point(form.capacityPoints,"假设容量"),requiredPoints:point(form.requiredPoints,"所需点数"),windowMinutes:Number(form.windowMinutes),charges}});
  return {sourceId:overview.source.id,expectedScope:overview.scopeToken,...(existing?{id:existing.id}:{}),expectedRevision:existing?.revision??null,config};
}
/** 只有用户点击示例按钮才填表；合成标签保留在可保存的标题与条件中，绝不自动提交。 */
export function syntheticMixtureForm(now=Date.now()):MixtureStudyForm {
  const f=newMixtureStudyForm(now);
  const matrix=[[1,0],[0,1],[1,1],[2,1],[1,2],[3,1],[1,3],[2,3],[3,2],[4,1],[1,4],[2,2]];
  return {...f,title:"合成示例（手动条件）",condition:{accountLabel:"合成账号标签，非官方身份",windowLabel:"合成手动点数窗口",effort:"合成 medium",speed:"合成标准档"},models:["合成模型 A","合成模型 B"],synthetic:true,
    samples:matrix.map(([a,b],i)=>({id:crypto.randomUUID(),fromAt:inputTime(new Date(now-(13-i)*3600000).toISOString()),toAt:inputTime(new Date(now-(13-i)*3600000+1800000).toISOString()),deltaPoints:String(a*2+b*5),tokens:[String(a*1000000),String(b*1000000)]})),
    historyCompleteFrom:inputTime(new Date(now-3600000).toISOString()),capacityPoints:"10",requiredPoints:"4",windowMinutes:"60",
    charges:[{id:crypto.randomUUID(),occurredAt:inputTime(new Date(now-900000).toISOString()),kind:"points",amountPoints:"4",tokens:["",""]},
      {id:crypto.randomUUID(),occurredAt:inputTime(new Date(now+900000).toISOString()),kind:"models",amountPoints:"",tokens:["500000","1000000"]}]};
}
export function changeMixtureModels(form:MixtureStudyForm,index:number,value:string|null):MixtureStudyForm {
  const transform=(values:string[])=>value===null?values.filter((_,i)=>i!==index):index===form.models.length?[...values,""]:values;
  return {...form,models:value===null?form.models.filter((_,i)=>i!==index):index===form.models.length?[...form.models,value]:form.models.map((v,i)=>i===index?value:v),
    samples:form.samples.map(s=>({...s,tokens:transform(s.tokens)})),charges:form.charges.map(c=>({...c,tokens:transform(c.tokens)}))};
}
