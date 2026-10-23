/** 手动条件实验：所有结果仅解释用户假设，不能授予官方额度或提醒资格。 */
export type ManualQuality = {
  quality: "conditional-manual";
  constraintsVerified: false;
  notifyEligible: false;
  uncertaintyBounds: null;
};
export interface MixtureSample {
  id: string;
  fromAt: string;
  toAt: string;
  deltaPoints: number;
  tokens: string[];
}
export type MixtureCharge = { id: string; occurredAt: string } & (
  { kind: "points"; amountPoints: number } |
  { kind: "models"; tokens: string[] }
);
export interface MixtureStudyConfig {
  title: string;
  condition: { accountLabel: string; windowLabel: string; effort: string; speed: string };
  models: string[];
  samples: MixtureSample[];
  rolling: {
    asOf: string;
    historyCompleteFrom: string | null;
    capacityPoints: number;
    requiredPoints: number;
    windowMinutes: number;
    charges: MixtureCharge[];
  };
}
export interface MixtureFitResult extends ManualQuality {
  kind: "separated" | "fixed_mix" | "unavailable";
  weights: { model: string; pointsPerMillionTokens: number }[] | null;
  fixedMix: { shares: number[]; pointsPerMillionTokens: number } | null;
  rank: number;
  conditionNumber: number | null;
  trainSamples: number;
  holdoutSamples: number;
  trainWape: number | null;
  holdoutWape: number | null;
  reasons: string[];
}
export interface RollingProjection extends ManualQuality {
  asOf: string;
  endsAt: string;
  points: {
    at: string;
    usedPoints: number | null;
    remainingPoints: number | null;
    available: boolean | null;
  }[];
  availabilityWindows: { fromAt: string; toAt: string }[];
  nextAvailableAt: string | null;
  reasons: string[];
}
export interface MixtureStudyProjection extends ManualQuality {
  fit: MixtureFitResult;
  rolling: RollingProjection;
  reasons: string[];
}
export interface MixtureStudy {
  id: string;
  revision: number;
  sourceId: string;
  configurationRevision: string;
  bindingState: "ready" | "pending_rebind" | "source_unavailable";
  createdAt: string;
  updatedAt: string;
  config: MixtureStudyConfig;
  projection: MixtureStudyProjection;
}
export interface MixtureStudiesOverview {
  schemaVersion: 1;
  revision: number;
  scopeToken: string;
  asOf: string;
  source: { id: string; clientKind: string; enabled: boolean; configurationRevision: string };
  studies: MixtureStudy[];
  reasons: string[];
}
export interface SaveMixtureStudy {
  sourceId: string;
  expectedScope: string;
  id?: string;
  expectedRevision: number | null;
  config: MixtureStudyConfig;
}
export interface DeleteMixtureStudy {
  sourceId: string;
  expectedScope: string;
  id: string;
  expectedRevision: number;
}

type Json = Record<string, unknown>;
export const MIXTURE_TOKEN_MAX = 9007199254740991n;
const object = (value: unknown): value is Json => !!value && typeof value === "object" && !Array.isArray(value);
const controls = /[\u0000-\u001f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/;
const text = (value: unknown, max = 512): value is string => typeof value === "string" && value.trim().length > 0 && value.length <= max && !controls.test(value);
const integer = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const finite = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0;
export const mixtureUuid = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value);
export const mixtureTime = (value: unknown): value is string => typeof value === "string" && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(value) && value.slice(0,4) !== "0000" && Number.isFinite(Date.parse(value)) && new Date(value).toISOString() === value;
export const mixtureTokens = (value: unknown): value is string => typeof value === "string" && /^(?:0|[1-9]\d{0,15})$/.test(value) && BigInt(value) <= MIXTURE_TOKEN_MAX;
const decimal = (value: unknown): value is string => typeof value === "string" && /^(?:0|[1-9]\d{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
const reasons = (value: unknown): value is string[] => Array.isArray(value) && value.length <= 128 && value.every(v => text(v, 2048));
function fail(message = "手动条件实验数据格式无效。"): never { throw Error(message); }
function array<T>(value: unknown, max: number, parse: (v: unknown) => T): T[] {
  if (!Array.isArray(value) || value.length > max) fail();
  return value.map(parse);
}
function flags(value: unknown): asserts value is Json & ManualQuality {
  if (!object(value) || value.quality !== "conditional-manual" || value.constraintsVerified !== false || value.notifyEligible !== false || value.uncertaintyBounds !== null)
    fail("手动实验不能被提升为官方容量、置信区间或通知资格。");
}
export function parseMixtureStudyConfig(value: unknown): MixtureStudyConfig {
  if (!object(value) || !text(value.title,80) || !object(value.condition) || !object(value.rolling)) fail();
  const condition = value.condition, rolling = value.rolling;
  if (![condition.accountLabel,condition.windowLabel,condition.effort,condition.speed].every(v => text(v,80))) fail("请提供完整的手动适用条件。");
  const models = array(value.models,3,v => text(v) ? v : fail());
  if (models.length < 1 || new Set(models).size !== models.length) fail("模型标签必须唯一，且为 1～3 个。");
  if (!mixtureTime(rolling.asOf) || !(rolling.historyCompleteFrom === null || mixtureTime(rolling.historyCompleteFrom))
      || (rolling.historyCompleteFrom !== null && Date.parse(rolling.historyCompleteFrom) > Date.parse(rolling.asOf))
      || !integer(rolling.windowMinutes) || rolling.windowMinutes < 1 || rolling.windowMinutes > 10080
      || !finite(rolling.capacityPoints) || rolling.capacityPoints <= 0 || rolling.capacityPoints > 1e9
      || !finite(rolling.requiredPoints) || rolling.requiredPoints <= 0 || rolling.requiredPoints > rolling.capacityPoints) fail("滑动窗口的手动条件无效。");
  const horizon = Date.parse(rolling.asOf) + rolling.windowMinutes * 120000;
  if (!Number.isFinite(horizon) || !mixtureTime(new Date(horizon).toISOString())) fail("模拟终点超过支持日期范围。");
  const ids = new Set<string>();
  const id = (v: unknown) => { if (!mixtureUuid(v) || ids.has(v)) fail("样本和进入记录须使用互不重复的 UUID。"); ids.add(v); return v; };
  const tokens = (v: unknown) => {
    const values = array(v,3,x => mixtureTokens(x) ? x : fail("Token 每列须显式填写非负安全整数字符串，不能以空白代替零。"));
    if (values.length !== models.length || !values.some(x => BigInt(x) > 0n)) fail("Token 列数须与模型相同，且至少一列大于零。");
    return values;
  };
  const samples = array(value.samples,128,v => {
    if (!object(v) || !mixtureTime(v.fromAt) || !mixtureTime(v.toAt) || Date.parse(v.fromAt) >= Date.parse(v.toAt)
        || Date.parse(v.toAt) > Date.parse(rolling.asOf as string) || !finite(v.deltaPoints) || v.deltaPoints > 1e9) fail("样本区间或手动增量无效。");
    return { id:id(v.id),fromAt:v.fromAt,toAt:v.toAt,deltaPoints:v.deltaPoints,tokens:tokens(v.tokens) };
  });
  if (samples.some((v,i) => i > 0 && Date.parse(v.fromAt) < Date.parse(samples[i-1].toAt))) fail("样本须按时间排列且不能重叠。");
  const charges = array(rolling.charges,128,v => {
    if (!object(v) || !mixtureTime(v.occurredAt) || Date.parse(v.occurredAt) > Date.parse(rolling.asOf as string) + (rolling.windowMinutes as number)*60000) fail("进入时间不得超过冻结时刻之后一个窗口。");
    const base = { id:id(v.id),occurredAt:v.occurredAt };
    if (v.kind === "models") return { ...base,kind:"models" as const,tokens:tokens(v.tokens) };
    if (v.kind !== "points" || !finite(v.amountPoints) || v.amountPoints <= 0 || v.amountPoints > 1e9) fail("手动进入点数须大于零且不超过 1e9。");
    return { ...base,kind:"points" as const,amountPoints:v.amountPoints };
  });
  return { title:value.title,condition:{accountLabel:condition.accountLabel as string,windowLabel:condition.windowLabel as string,effort:condition.effort as string,speed:condition.speed as string},models,samples,
    rolling:{asOf:rolling.asOf,historyCompleteFrom:rolling.historyCompleteFrom,capacityPoints:rolling.capacityPoints,requiredPoints:rolling.requiredPoints,windowMinutes:rolling.windowMinutes,charges} };
}
function fit(value: unknown, models: string[]): MixtureFitResult {
  flags(value);
  if (!["separated","fixed_mix","unavailable"].includes(String(value.kind)) || !integer(value.rank) || value.rank > models.length || !integer(value.trainSamples) || !integer(value.holdoutSamples)
      || value.trainSamples + value.holdoutSamples > 128 || !reasons(value.reasons)) fail();
  for (const field of ["conditionNumber","trainWape","holdoutWape"]) if (value[field] !== null && !finite(value[field])) fail();
  const weights = value.weights === null ? null : array(value.weights,3,v => {
    if (!object(v) || !text(v.model) || !finite(v.pointsPerMillionTokens)) fail();
    return {model:v.model,pointsPerMillionTokens:v.pointsPerMillionTokens};
  });
  let fixedMix: MixtureFitResult["fixedMix"] = null;
  if (value.fixedMix !== null) {
    if (!object(value.fixedMix) || !finite(value.fixedMix.pointsPerMillionTokens)) fail();
    const shares = array(value.fixedMix.shares,3,v => finite(v) && v <= 1 ? v : fail());
    if (shares.length !== models.length || Math.abs(shares.reduce((n,v)=>n+v,0)-1) > 1e-6) fail();
    fixedMix = {shares,pointsPerMillionTokens:value.fixedMix.pointsPerMillionTokens};
  }
  if (value.kind === "separated" ? !weights || weights.length !== models.length || new Set(weights.map(v=>v.model)).size !== models.length || weights.some(v=>!models.includes(v.model)) || fixedMix !== null
      : value.kind === "fixed_mix" ? weights !== null || fixedMix === null : weights !== null || fixedMix !== null) fail("拟合结果与可辨识状态矛盾。");
  if (value.kind !== "unavailable" && (value.trainSamples < models.length+2 || value.holdoutSamples < 2 || value.holdoutWape === null || (value.holdoutWape as number) > 0.20)) fail("样本或留出依据不足，不能发布拟合系数。");
  if (value.kind === "separated" && value.rank !== models.length || value.kind === "fixed_mix" && (value.rank !== 1 || models.length < 2)) fail("拟合系数与矩阵可辨识性矛盾。");
  return {quality:"conditional-manual",constraintsVerified:false,notifyEligible:false,uncertaintyBounds:null,
    kind:value.kind as MixtureFitResult["kind"],weights,fixedMix,rank:value.rank,conditionNumber:value.conditionNumber as number|null,
    trainSamples:value.trainSamples,holdoutSamples:value.holdoutSamples,trainWape:value.trainWape as number|null,holdoutWape:value.holdoutWape as number|null,reasons:value.reasons};
}
function rolling(value: unknown, config: MixtureStudyConfig): RollingProjection {
  flags(value);
  if (!mixtureTime(value.asOf) || !mixtureTime(value.endsAt) || value.asOf !== config.rolling.asOf
      || Date.parse(value.endsAt) !== Date.parse(value.asOf)+config.rolling.windowMinutes*120000 || !reasons(value.reasons)) fail("模拟曲线的冻结范围不一致。");
  const start=Date.parse(value.asOf),end=Date.parse(value.endsAt);
  const within=(v: unknown):v is string => mixtureTime(v) && Date.parse(v)>=start && Date.parse(v)<=end;
  const points=array(value.points,260,v=>{
    if (!object(v) || !within(v.at)) fail();
    const unknown = v.usedPoints===null && v.remainingPoints===null && v.available===null;
    if (!unknown && (!finite(v.usedPoints) || !finite(v.remainingPoints) || typeof v.available!=="boolean")) fail("未知占用、余额和可用性必须同时为空。");
    return {at:v.at,usedPoints:v.usedPoints as number|null,remainingPoints:v.remainingPoints as number|null,available:v.available as boolean|null};
  });
  if (points.length<2 || points[0].at!==value.asOf || points.at(-1)!.at!==value.endsAt || points.some((v,i)=>i>0&&Date.parse(v.at)<=Date.parse(points[i-1].at))) fail("曲线必须有唯一、有序的事件点及两个边界。");
  const availabilityWindows=array(value.availabilityWindows,260,v=>{
    if (!object(v)||!within(v.fromAt)||!within(v.toAt)||Date.parse(v.fromAt)>=Date.parse(v.toAt)) fail("可用区间须有界且不能超出模拟范围。");
    return {fromAt:v.fromAt,toAt:v.toAt};
  });
  if (availabilityWindows.some((v,i)=>i>0&&Date.parse(v.fromAt)<Date.parse(availabilityWindows[i-1].toAt))) fail("可用时段不能重叠或乱序。");
  if (!(value.nextAvailableAt===null||within(value.nextAvailableAt))) fail();
  return {quality:"conditional-manual",constraintsVerified:false,notifyEligible:false,uncertaintyBounds:null,asOf:value.asOf,endsAt:value.endsAt,points,availabilityWindows,nextAvailableAt:value.nextAvailableAt,reasons:value.reasons};
}
export function parseMixtureStudiesOverview(value: unknown, sourceId?: string): MixtureStudiesOverview {
  if (!object(value)||value.schemaVersion!==1||!integer(value.revision)||!text(value.scopeToken,2048)||!mixtureTime(value.asOf)||!object(value.source)
      ||!text(value.source.id)||(sourceId!==undefined&&value.source.id!==sourceId)||!text(value.source.clientKind)||typeof value.source.enabled!=="boolean"||!decimal(value.source.configurationRevision)||!reasons(value.reasons)) fail();
  const source=value.source,ids=new Set<string>();
  const studies=array(value.studies,32,v=>{
    if (!object(v)||!mixtureUuid(v.id)||ids.has(v.id)||v.sourceId!==source.id||!integer(v.revision)||!decimal(v.configurationRevision)
        ||!["ready","pending_rebind","source_unavailable"].includes(String(v.bindingState))||!mixtureTime(v.createdAt)||!mixtureTime(v.updatedAt)||!object(v.projection)) fail();
    ids.add(v.id);const config=parseMixtureStudyConfig(v.config),projection=v.projection;flags(projection);
    if (!reasons(projection.reasons)) fail();
    const fitValue=fit(projection.fit,config.models),rollingValue=rolling(projection.rolling,config);
    if (v.bindingState!=="ready" && (fitValue.kind!=="unavailable"||rollingValue.points.some(p=>p.usedPoints!==null)||rollingValue.availabilityWindows.length||rollingValue.nextAvailableAt!==null)) fail("旧来源绑定不能发布当前可用结果。");
    if (v.bindingState==="ready"&&v.configurationRevision!==source.configurationRevision) fail("来源配置修订不匹配。");
    return {id:v.id,revision:v.revision,sourceId:source.id as string,configurationRevision:v.configurationRevision,bindingState:v.bindingState as MixtureStudy["bindingState"],createdAt:v.createdAt,updatedAt:v.updatedAt,config,
      projection:{quality:"conditional-manual" as const,constraintsVerified:false as const,notifyEligible:false as const,uncertaintyBounds:null,fit:fitValue,rolling:rollingValue,reasons:projection.reasons}};
  });
  return {schemaVersion:1,revision:value.revision,scopeToken:value.scopeToken,asOf:value.asOf,source:{id:source.id as string,clientKind:source.clientKind as string,enabled:source.enabled as boolean,configurationRevision:source.configurationRevision as string},studies,reasons:value.reasons};
}


