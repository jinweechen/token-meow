import { useMemo } from "react";
import { observedUsd, parseObservedInsights, type ObservedEvent } from "../../shared/observedInsights";
import { usageTime } from "../../shared/usageDashboard";
import "./observedInsights.css";
const label=(kind:string)=>({cache_usage:"缓存记录",settings_snapshot:"配置起点",settings_change:"配置变化",compaction:"上下文压缩"}[kind]??kind);
const field=(key:string)=>({model:"模型",providerId:"供应商",effort:"努力程度",speed:"速度档"}[key]??key);
const number=(v:string|null|undefined)=>v==null?"未知":v;
function eventDetail(e:ObservedEvent):string {
  if(e.kind==="cache_usage")return `实际模型 ${e.observedModel??"未知"} · 输入 ${number(e.inputTokens)} · 缓存读取 ${number(e.cachedInputTokens)}`;
  if(e.kind==="compaction")return `已记录上下文压缩${e.windowNumber==null?"":` · 窗口序号 ${e.windowNumber}`}，不读取压缩正文。`;
  return Object.entries(e.condition??{}).map(([key,value])=>`${field(key)} ${value??"未知"}`).join(" · ");
}
export function ObservedInsightsPanel({value,privacyMode}:{value:unknown;privacyMode:boolean}) {
  const result=useMemo(()=>{try{return{data:parseObservedInsights(value),error:null};}catch{return{data:null,error:"本地观察摘要暂不可用，请刷新当前用量范围。"};}},[value]);
  if(value==null)return null;
  const d=result.data;
  if(!d)return <p role="status" className="observed-note">{result.error}</p>;
  return <section className="observed-insights" aria-label="本地观察摘要与事件">
    <details><summary><strong>每日摘要与缓存</strong><span>{d.from} ～ {d.to}</span></summary>
      <p className="observed-note">摘要由本地记录生成，无额外模型调用。缓存读取比例只覆盖输入和缓存字段均有效的记录，不能证明实际总使用或调用质量。</p>
      <div className="observed-metrics"><span>缓存读取比例 <b>{d.cache.cacheReadPercent==null?"未知":`${d.cache.cacheReadPercent.toFixed(1)}%`}</b></span><span>有效字段 {d.cache.knownResponses} 条</span><span>字段未知 {d.cache.unknownResponses} 条</span></div>
      <div className="observed-daily">{d.daily.map(day=><article key={day.day}><p>{day.text}</p><small>已记录 {number(day.recordedTokens)} Tokens · 估算成本 {observedUsd(day.estimatedCostUsd)}（已定价 {day.pricedResponses} 条）</small></article>)}</div>
    </details>
    {!privacyMode&&<details><summary><strong>缓存、配置与压缩时间线</strong><span>{d.timeline.length} 条{d.timelineTruncated?" · 已截断":""}</span></summary>
      <p className="observed-note">配置和压缩记录从此版本开始采集；旧日志需显式导入历史。兼容通知与投影条目不会重复计为一次压缩。{d.timelineTruncated?"候选扫描或展示已截断，当前筛选可能只包含部分匹配事件。":""}</p>
      {!d.timeline.length?<p>当前范围没有已记录事件。</p>:<ol className="observed-timeline">{d.timeline.map(e=><li key={`${e.kind}:${e.id}`}><time>{usageTime(e.eventTime,true)}</time><b>{label(e.kind)}</b><span>{eventDetail(e)}</span></li>)}</ol>}
    </details>}
    {!privacyMode&&<details><summary><strong>模型条件样本</strong><span>实际响应模型 · 不作质量排名</span></summary>
      <p className="observed-note">相同来源、供应商、账户、努力程度、速度档、输入数量级和缓存比例区间才标为可对照；每个模型至少 5 个样本。回合首 Token 是回合计时，不能当作模型生成速度。观测均值不能说明模型导致了差异。</p>
      <p className="observed-note">实际模型或输入/缓存字段不明的记录：{d.excludedComparisonResponses} 条{d.comparisonTruncated?"；条件组已截断":""}。</p>
      {!d.comparisons.length?<p>当前没有可描述的实际模型样本。</p>:d.comparisons.map((group,index)=><article className="observed-comparison" key={index}>
        <p><strong>{group.comparable?"条件已知且样本达标":"条件或样本不足，仅作记录描述"}</strong> · {group.condition.providerId} · 努力 {group.condition.effort} · 速度 {group.condition.speed} · 输入 10^{group.condition.inputMagnitude} 档 · 缓存 {group.condition.cacheDecile*10}～{(group.condition.cacheDecile+1)*10}%</p>
        <div className="observed-scroll"><table><thead><tr><th>实际响应模型</th><th>样本</th><th>回合首 Token 均值</th><th>估算每条成本均值</th><th>缓存读取</th></tr></thead><tbody>{group.models.map(m=><tr key={m.model}><td>{m.model}</td><td>{m.samples}{!m.eligible?" · 不足 5":""}</td><td>{m.meanTurnTimeToFirstTokenMs==null?"未知":`${m.meanTurnTimeToFirstTokenMs.toFixed(0)} ms`}（{m.timedSamples} 样本）</td><td>{m.meanEstimatedCostUsd===null?"未知":observedUsd(m.meanEstimatedCostUsd)}（{m.pricedSamples} 样本）</td><td>{m.cache.cacheReadPercent==null?"未知":`${m.cache.cacheReadPercent.toFixed(1)}%`}</td></tr>)}</tbody></table></div>
      </article>)}
    </details>}
  </section>;
}
