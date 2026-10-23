export interface ObservedCache { inputTokens:string|null; cachedInputTokens:string|null; cacheReadPercent:number|null; knownResponses:number; unknownResponses:number }
export interface ObservedEvent { id:string; kind:"cache_usage"|"settings_snapshot"|"settings_change"|"compaction"; eventTime:string; sourceId:string; threadId:string;
  observedModel?:string|null; inputTokens?:string|null; cachedInputTokens?:string|null; condition?:Record<string,string|null>; changedFields?:string[]; windowId?:string|null; windowNumber?:string|null }
export interface ObservedInsights {
  schemaVersion:1; from:string; to:string; cache:ObservedCache;
  daily:{day:string;text:string;recordedTokens:string|null;responses:number;cache:ObservedCache;estimatedCostUsd:string|null;pricedResponses:number}[];
  timeline:ObservedEvent[];timelineTruncated:boolean;comparisonTruncated:boolean;excludedComparisonResponses:number;
  comparisons:{condition:{sourceId:string;providerId:string;accountScope:string|null;known:boolean;effort:string;speed:string;inputMagnitude:number;cacheDecile:number};
    comparable:boolean;models:{model:string;samples:number;eligible:boolean;cache:ObservedCache;timedSamples:number;meanTurnTimeToFirstTokenMs:number|null;pricedSamples:number;meanEstimatedCostUsd:string|null}[];reasons:string[]}[];
}
const obj=(v:unknown):v is Record<string,unknown>=>!!v&&typeof v==="object"&&!Array.isArray(v);
const txt=(v:unknown,max=2048):v is string=>typeof v==="string"&&v.length>0&&v.length<=max;
const nat=(v:unknown):v is number=>typeof v==="number"&&Number.isSafeInteger(v)&&v>=0;
const decimal=(v:unknown):v is string=>typeof v==="string"&&/^(0|[1-9]\d*)(\.\d+)?$/.test(v)&&v.length<=128;
const nullable=(v:unknown)=>v===null||decimal(v);
const countText=(v:unknown)=>v===null||(typeof v==="string"&&/^(0|[1-9]\d*)$/.test(v)&&v.length<=128);
const finite=(v:unknown):v is number=>typeof v==="number"&&Number.isFinite(v)&&v>=0;
// 原生事件带明确时区；不能由浏览器按运行机器本地时区猜测。
const date=(v:unknown):v is string=>typeof v==="string"&&/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})$/.test(v)&&Number.isFinite(Date.parse(v));
const day=(v:unknown):v is string=>typeof v==="string"&&/^\d{4}-\d{2}-\d{2}$/.test(v)&&Number.isFinite(Date.parse(`${v}T00:00:00Z`))&&new Date(`${v}T00:00:00Z`).toISOString().slice(0,10)===v;
/** 成本来自原生精确十进制；展示分组和补两位不改变数值。 */
export function observedUsd(value:string|null|undefined):string {
  if(value==null||value==="")return "未定价";
  if(!decimal(value))return "未提供";
  const [whole,fraction=""]=value.split(".");
  const digits=fraction.replace(/0+$/,"").padEnd(2,"0");
  return `$${whole.replace(/\B(?=(\d{3})+(?!\d))/g,",")}.${digits}`;
}
function cache(v:unknown):ObservedCache {
  if(!obj(v)||!countText(v.inputTokens)||!countText(v.cachedInputTokens)||!(v.cacheReadPercent===null||(finite(v.cacheReadPercent)&&v.cacheReadPercent<=100))||!nat(v.knownResponses)||!nat(v.unknownResponses))throw Error("缓存统计格式无效。");
  if(v.knownResponses===0&&(v.inputTokens!==null||v.cachedInputTokens!==null||v.cacheReadPercent!==null))throw Error("未知缓存不能发布已知值。");
  if(v.knownResponses>0&&(v.inputTokens===null||v.cachedInputTokens===null))throw Error("有效缓存须同时提供输入和缓存子集。");
  if(v.inputTokens!==null&&v.cachedInputTokens!==null&&BigInt(v.cachedInputTokens as string)>BigInt(v.inputTokens as string))throw Error("缓存子集超过输入。");
  if(v.inputTokens==="0"&&v.cacheReadPercent!==null)throw Error("零输入没有可计算的缓存比例。");
  return v as unknown as ObservedCache;
}
export function parseObservedInsights(v:unknown):ObservedInsights {
  if(!obj(v)||v.schemaVersion!==1||!day(v.from)||!day(v.to)||v.from>v.to||!Array.isArray(v.daily)||v.daily.length>366||!Array.isArray(v.timeline)||v.timeline.length>256||!Array.isArray(v.comparisons)||v.comparisons.length>128||typeof v.timelineTruncated!=="boolean"||typeof v.comparisonTruncated!=="boolean"||!nat(v.excludedComparisonResponses))throw Error("观察摘要格式无效。");
  const daily=v.daily.map(d=>{if(!obj(d)||!day(d.day)||!txt(d.text)||!countText(d.recordedTokens)||!obj(d.usage)||!nat(d.usage.responseCount)||!nullable(d.estimatedCostUsd)||!nat(d.pricedResponses)||d.pricedResponses>d.usage.responseCount)throw Error("每日摘要格式无效。");return{day:d.day,text:d.text,recordedTokens:d.recordedTokens as string|null,responses:d.usage.responseCount,cache:cache(d.cache),estimatedCostUsd:d.estimatedCostUsd as string|null,pricedResponses:d.pricedResponses};});
  const timeline=v.timeline.map(e=>{
    if(!obj(e)||!txt(e.id,128)||!["cache_usage","settings_snapshot","settings_change","compaction"].includes(String(e.kind))||!date(e.eventTime)||!txt(e.sourceId,512)||!txt(e.threadId,512))throw Error("事件时间线格式无效。");
    if(e.kind==="cache_usage"&&(!(e.observedModel===null||txt(e.observedModel,512))||!countText(e.inputTokens)||!countText(e.cachedInputTokens)))throw Error("缓存事件格式无效。");
    if(e.kind==="settings_snapshot"||e.kind==="settings_change") {
      if(!obj(e.condition)||Object.keys(e.condition).some(k=>!["model","providerId","effort","speed"].includes(k))||Object.values(e.condition).some(x=>!(x===null||txt(x,512)))||!Array.isArray(e.changedFields)||e.changedFields.some(x=>typeof x!=="string"))throw Error("配置事件格式无效。");
    }
    if(e.kind==="compaction"&&(!(e.windowId===null||txt(e.windowId,128))||!countText(e.windowNumber)))throw Error("压缩窗口身份无效。");
    return e as unknown as ObservedEvent;
  });
  const comparisons=v.comparisons.map(g=>{
    if(!obj(g)||!obj(g.condition)||typeof g.comparable!=="boolean"||!Array.isArray(g.models)||g.models.length>64||g.rank!==null||g.quality!=="conditional-observed"||!Array.isArray(g.reasons)||g.reasons.some(x=>typeof x!=="string"))throw Error("模型样本格式无效。");
    const c=g.condition;
    if(![c.sourceId,c.providerId,c.effort,c.speed].every(x=>txt(x,512))||!(c.accountScope===null||txt(c.accountScope,128))||typeof c.known!=="boolean"||!nat(c.inputMagnitude)||!nat(c.cacheDecile)||c.cacheDecile>9||(g.comparable&&!c.known))throw Error("模型条件格式无效。");
    if(c.known&&(c.accountScope===null||[c.providerId,c.effort,c.speed].some(x=>x==="unknown")))throw Error("条件未知时不能仅凭标记成为可对照。");
    const names=new Set<string>();
    const models=g.models.map((m):ObservedInsights["comparisons"][number]["models"][number]=>{if(!obj(m)||!txt(m.model,512)||names.has(m.model)||!nat(m.samples)||m.eligible!==(m.samples>=5)||!nat(m.timedSamples)||m.timedSamples>m.samples||!(m.meanTurnTimeToFirstTokenMs===null||finite(m.meanTurnTimeToFirstTokenMs))||!nat(m.pricedSamples)||m.pricedSamples>m.samples||!nullable(m.meanEstimatedCostUsd)||(m.timedSamples<5&&m.meanTurnTimeToFirstTokenMs!==null)||(m.pricedSamples<5&&m.meanEstimatedCostUsd!==null))throw Error("模型计量格式无效。");names.add(m.model);return{model:m.model,samples:m.samples,eligible:m.samples>=5,cache:cache(m.cache),timedSamples:m.timedSamples,meanTurnTimeToFirstTokenMs:m.meanTurnTimeToFirstTokenMs as number|null,pricedSamples:m.pricedSamples,meanEstimatedCostUsd:m.meanEstimatedCostUsd as string|null};});
    if(g.comparable&&models.filter(m=>m.eligible===true).length<2)throw Error("对照资格缺少足够模型样本。");
    return{condition:c,comparable:g.comparable,models,reasons:g.reasons} as unknown as ObservedInsights["comparisons"][number];
  });
  return {schemaVersion:1,from:v.from,to:v.to,cache:cache(v.cache),daily,timeline,timelineTruncated:v.timelineTruncated,comparisonTruncated:v.comparisonTruncated,excludedComparisonResponses:v.excludedComparisonResponses,comparisons};
}
