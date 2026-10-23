import { useState } from "react";
import type { MixtureStudyConfig, RollingProjection } from "../../shared/mixtureStudyContracts";
import { mixtureStudyGeometry } from "../../shared/mixtureStudyGeometry";
import { subscriptionTime } from "../../shared/subscriptionPresentation";
import { mixtureStudyTime } from "../../shared/mixtureStudyPresentation";

export const mixtureNumber=(v:number|null)=>v===null?"未知":v>0&&(v<0.000001||v>1e15)?v.toExponential(5):new Intl.NumberFormat("zh-CN",{maximumFractionDigits:6}).format(v);
export function MixtureStudyCurve({value,config}:{value:RollingProjection;config:MixtureStudyConfig}) {
  const [selected,setSelected]=useState<string|null>(null);
  const width=860,height=236,plot={left:64,top:22,width:776,height:156};
  const geometry=mixtureStudyGeometry(value,config.rolling.capacityPoints,plot);
  const point=value.points.find(p=>p.at===selected)??null;
  return <section className="mixture-study-curve" aria-label="手动滑动释放场景">
    <h4>手动滑动释放场景</h4><p className="mixture-study-note">冻结于 {mixtureStudyTime(value.asOf)}，模拟截至 {mixtureStudyTime(value.endsAt)}（上海）。阶梯表示同时处理进入与到期，灰色区间未知；范围以外不作判断。</p>
    <div className="mixture-study-chart-scroll"><svg viewBox={`0 0 ${width} ${height}`} className="mixture-study-chart" role="img" aria-label="手动占用阶梯图，灰色区间占用未知" data-time-start={value.asOf} data-time-end={value.endsAt}>
      {value.availabilityWindows.map(w=><rect key={w.fromAt} x={geometry.x(w.fromAt)} y={plot.top} width={geometry.x(w.toAt)-geometry.x(w.fromAt)} height={plot.height} className="mixture-study-chart-available"/>) }
      {geometry.unknown.map((r,i)=><rect key={i} x={r.left} y={plot.top} width={r.width} height={plot.height} className="mixture-study-chart-unknown" data-unknown="true"/>)}
      {[0,0.5,1].map(f=><g key={f}><line x1={plot.left} x2={plot.left+plot.width} y1={plot.top+plot.height*(1-f)} y2={plot.top+plot.height*(1-f)} className="mixture-study-chart-grid"/><text x={plot.left-8} y={plot.top+plot.height*(1-f)+4} textAnchor="end">{mixtureNumber(geometry.max*f)}</text></g>)}
      <line x1={plot.left} x2={plot.left+plot.width} y1={geometry.capacityY} y2={geometry.capacityY} className="mixture-study-chart-capacity"/>
      {geometry.paths.map((path,i)=><path key={i} d={path} className="mixture-study-chart-step" data-step-after="true"/>)}
      {geometry.coordinates.map(({point:p,x,y})=><g key={p.at} tabIndex={0} role="img" aria-label={`${subscriptionTime(p.at)}，占用 ${mixtureNumber(p.usedPoints)} 手动点`} onMouseEnter={()=>setSelected(p.at)} onMouseLeave={()=>setSelected(null)} onFocus={()=>setSelected(p.at)} onBlur={()=>setSelected(null)}><title>{p.at} · 占用 {mixtureNumber(p.usedPoints)} · 手动剩余 {mixtureNumber(p.remainingPoints)}</title><rect x={x-5} y={plot.top} width="10" height={plot.height} fill="transparent"/>{y!==null&&<circle cx={x} cy={y} r={selected===p.at?4:2.5} className="mixture-study-chart-point"/>}</g>)}
      {[0,0.25,0.5,0.75,1].map(f=><text key={f} x={plot.left+plot.width*f} y="205" textAnchor={f===0?"start":f===1?"end":"middle"}>{subscriptionTime(new Date(Date.parse(value.asOf)+(Date.parse(value.endsAt)-Date.parse(value.asOf))*f).toISOString())}</text>)}
      <text x="64" y="228">手动扣额点 · 实线占用 / 虚线假设容量 / 浅绿满足所需点数 / 灰色未知</text>
    </svg></div>
    <p className="mixture-study-chart-caption" role="status">{point?`${mixtureStudyTime(point.at)} · 占用 ${mixtureNumber(point.usedPoints)} · 手动剩余 ${mixtureNumber(point.remainingPoints)} · ${point.available===null?"可用性未知":point.available?"满足本次所需":"暂不满足本次所需"}`:"可用鼠标或键盘查看事件点；图中数值仅来自已保存的手动场景。"}</p>
    <p>在冻结条件下首次满足所需点数：<strong>{value.nextAvailableAt?mixtureStudyTime(value.nextAvailableAt):"模拟范围内暂不可确认"}</strong></p>
    <p className="mixture-study-note">未知历史过期后的已知部分，也只表示你声明的未来输入情景，不证明现实中没有其他消耗。</p>
    <details><summary>查看模拟事件点（{value.points.length} 个）</summary><div className="mixture-study-table-wrap"><table><thead><tr><th>上海时间</th><th>占用手动点</th><th>剩余手动点</th><th>满足所需</th></tr></thead><tbody>{value.points.map(p=><tr key={p.at}><td>{mixtureStudyTime(p.at)}</td><td>{mixtureNumber(p.usedPoints)}</td><td>{mixtureNumber(p.remainingPoints)}</td><td>{p.available===null?"未知":p.available?"是":"否"}</td></tr>)}</tbody></table></div></details>
  </section>;
}
