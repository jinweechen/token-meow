import { useState } from "react";
import type { MixtureStudiesOverview, MixtureStudy, SaveMixtureStudy } from "../../shared/mixtureStudyContracts";
import { buildMixtureStudyInput, changeMixtureModels, mixtureFormFromStudy, newMixtureStudyForm, syntheticMixtureForm, type MixtureStudyForm } from "../../shared/mixtureStudyForms";
import { localScheduleDateInput } from "../../shared/localScheduleForms";

export function MixtureStudyEditor({overview,existing,disabled,onSave,onCancel}:{overview:MixtureStudiesOverview;existing:MixtureStudy|null;disabled:boolean;onSave:(input:SaveMixtureStudy)=>Promise<boolean>;onCancel:()=>void}) {
  const [form,setForm]=useState(()=>existing?mixtureFormFromStudy(existing):newMixtureStudyForm());
  const [error,setError]=useState<string|null>(null),[unconfirmed,setUnconfirmed]=useState(false),[checked,setChecked]=useState(false);
  const update=<K extends keyof MixtureStudyForm>(key:K,value:MixtureStudyForm[K])=>{setForm(f=>({...f,[key]:value}));setError(null);};
  async function save(){if(disabled||(unconfirmed&&!checked))return;setError(null);try{if(!await onSave(buildMixtureStudyInput(form,overview,existing))){setError("保存尚未确认。请核对更新后的实验，输入不会自动重发。");if(!existing){setUnconfirmed(true);setChecked(false);}}}catch(e){setError(e instanceof Error?e.message:"手动条件校验失败。");}}
  return <section className="mixture-study-editor" aria-labelledby="mixture-study-editor-title"><div className="mixture-study-heading"><h3 id="mixture-study-editor-title">{existing?"修改手动条件实验":"新建手动条件实验"}</h3><button type="button" disabled={disabled} onClick={onCancel}>取消实验编辑</button></div>
    <p className="mixture-study-note">所有标签、历史增量和未来消耗均由你填写，不代表官方账号、实际模型路由或厂商窗口规则。修改输入期间旧结果暂停展示，保存后重新计算。</p>
    <div className="mixture-study-actions"><button type="button" disabled={disabled} onClick={()=>{setForm(syntheticMixtureForm());setError(null);}}>填入合成示例（不会保存）</button>{form.synthetic&&<span className="mixture-study-tag">合成样例 · 用户手动情景</span>}</div>
    <form onSubmit={event=>{event.preventDefault();void save();}}><fieldset disabled={disabled}>
      <div className="mixture-study-grid"><label>实验名称<input required maxLength={80} value={form.title} onChange={e=>update("title",e.target.value)}/></label>
        {([["accountLabel","账户条件标签"],["windowLabel","窗口条件标签"],["effort","推理档位条件"],["speed","速度档位条件"]] as const).map(([key,label])=><label key={key}>{label}<input required maxLength={80} value={form.condition[key]} placeholder="用户假设，非官方身份" onChange={e=>update("condition",{...form.condition,[key]:e.target.value})}/></label>)}</div>
      <h4>模型标签与样本列</h4><p className="mixture-study-note">最多 3 个模型标签。每列填写互不重复的 Token 总量，缓存和推理等包含项不能再次相加；空白不会当成零。</p>
      <div className="mixture-study-models">{form.models.map((model,index)=><div key={index}><label>模型 {index+1} 标签<input required maxLength={512} value={model} onChange={e=>{setForm(f=>changeMixtureModels(f,index,e.target.value));setError(null);}}/></label><button type="button" disabled={form.models.length===1} aria-label={`删除模型 ${index+1}`} onClick={()=>setForm(f=>changeMixtureModels(f,index,null))}>删除列</button></div>)}</div>
      <button type="button" disabled={form.models.length>=3} onClick={()=>setForm(f=>changeMixtureModels(f,f.models.length,""))}>增加模型列</button>
      <details className="mixture-study-input-section" open><summary>历史条件样本（{form.samples.length} 条）</summary><p className="mixture-study-note">每条是一个不重叠时间区间内的手动扣额点增量。Core 固定将时间上最后 max(2, 向上取整样本数/4) 条留出，不由前端挑选好看的样本。</p>
        <div className="mixture-study-table-wrap"><table><thead><tr><th>开始（上海）</th><th>结束（上海）</th><th>手动点增量</th>{form.models.map((m,i)=><th key={i}>{m||`模型 ${i+1}`} Token</th>)}<th>操作</th></tr></thead><tbody>{form.samples.map((sample,index)=><tr key={sample.id}>
          <td><input aria-label={`样本 ${index+1} 开始`} type="datetime-local" step="0.001" required value={sample.fromAt} onChange={e=>update("samples",form.samples.map((v,i)=>i===index?{...v,fromAt:e.target.value}:v))}/></td>
          <td><input aria-label={`样本 ${index+1} 结束`} type="datetime-local" step="0.001" required value={sample.toAt} onChange={e=>update("samples",form.samples.map((v,i)=>i===index?{...v,toAt:e.target.value}:v))}/></td>
          <td><input aria-label={`样本 ${index+1} 手动点增量`} inputMode="decimal" maxLength={48} required value={sample.deltaPoints} onChange={e=>update("samples",form.samples.map((v,i)=>i===index?{...v,deltaPoints:e.target.value}:v))}/></td>
          {form.models.map((_,column)=><td key={column}><input aria-label={`样本 ${index+1} 模型 ${column+1} Token`} inputMode="numeric" maxLength={16} required value={sample.tokens[column]} onChange={e=>update("samples",form.samples.map((v,i)=>i===index?{...v,tokens:v.tokens.map((n,j)=>j===column?e.target.value:n)}:v))}/></td>)}
          <td><button type="button" aria-label={`删除样本 ${index+1}`} onClick={()=>update("samples",form.samples.filter((_,i)=>i!==index))}>删除</button></td></tr>)}</tbody></table></div>
        {!form.samples.length&&<p className="mixture-study-note">尚无样本，模型关系将保持不可估；直接点数的手动窗口仍可模拟。</p>}
        <button type="button" disabled={form.samples.length>=128} onClick={()=>update("samples",[...form.samples,{id:crypto.randomUUID(),fromAt:"",toAt:"",deltaPoints:"",tokens:form.models.map(()=>"")}])}>添加历史样本</button>
      </details>
      <h4>手动滑动窗口条件</h4><p className="mixture-study-note">使用量按进入时刻留在窗口内，到期再释放；此处的时长和容量均为手动假设，不读取官方窗口。基准时刻冻结，普通刷新不会把曲线推进到现在。</p>
      <div className="mixture-study-grid"><label>冻结基准（上海时间）<input type="datetime-local" step="0.001" required value={form.asOf} onChange={e=>update("asOf",e.target.value)}/></label><label>历史完整起点（可留空表示未知）<input type="datetime-local" step="0.001" value={form.historyCompleteFrom} onChange={e=>update("historyCompleteFrom",e.target.value)}/></label>
        <label>假设容量（手动扣额点）<input inputMode="decimal" maxLength={48} required value={form.capacityPoints} onChange={e=>update("capacityPoints",e.target.value)}/></label><label>下一次所需（手动扣额点）<input inputMode="decimal" maxLength={48} required value={form.requiredPoints} onChange={e=>update("requiredPoints",e.target.value)}/></label><label>滑动时长（分钟）<input inputMode="numeric" maxLength={5} required value={form.windowMinutes} onChange={e=>update("windowMinutes",e.target.value)}/></label></div>
      <button type="button" onClick={()=>update("asOf",localScheduleDateInput(new Date().toISOString(),"milliseconds"))}>将冻结基准设为当前时刻</button>
      <details className="mixture-study-input-section" open><summary>历史与计划进入记录（{form.charges.length} 条）</summary><p className="mixture-study-note">计划进入最晚为基准之后一个窗口，模拟到两个窗口后结束。模型 Token 仅在关系可区分且留出通过时折算；固定组合、零系数或缺失历史不会被当作免费或零占用。</p>
        <div className="mixture-study-charge-list">{form.charges.map((charge,index)=><div className="mixture-study-charge" key={charge.id}><div className="mixture-study-grid"><label>进入 {index+1} 时间（上海）<input type="datetime-local" step="0.001" required value={charge.occurredAt} onChange={e=>update("charges",form.charges.map((v,i)=>i===index?{...v,occurredAt:e.target.value}:v))}/></label>
          <label>进入 {index+1} 类型<select value={charge.kind} onChange={e=>update("charges",form.charges.map((v,i)=>i===index?{...v,kind:e.target.value as "points"|"models"}:v))}><option value="models">模型 Token 条件换算</option><option value="points">直接填写手动扣额点</option></select></label>
          {charge.kind==="points"?<label>进入 {index+1} 手动点数<input required inputMode="decimal" maxLength={48} value={charge.amountPoints} onChange={e=>update("charges",form.charges.map((v,i)=>i===index?{...v,amountPoints:e.target.value}:v))}/></label>:form.models.map((m,column)=><label key={column}>进入 {index+1} 模型 {column+1} Token · {m||"标签待填写"}<input required inputMode="numeric" maxLength={16} value={charge.tokens[column]} onChange={e=>update("charges",form.charges.map((v,i)=>i===index?{...v,tokens:v.tokens.map((n,j)=>j===column?e.target.value:n)}:v))}/></label>)}</div><button type="button" aria-label={`删除进入记录 ${index+1}`} onClick={()=>update("charges",form.charges.filter((_,i)=>i!==index))}>删除进入记录</button></div>)}</div>
        <button type="button" disabled={form.charges.length>=128} onClick={()=>update("charges",[...form.charges,{id:crypto.randomUUID(),occurredAt:form.asOf,kind:"models",amountPoints:"",tokens:form.models.map(()=>"")}])}>添加进入记录</button>
      </details>
    </fieldset>{error&&<p className="mixture-study-error" role="alert">{error}</p>}
    {unconfirmed&&<label className="mixture-study-checkbox"><input type="checkbox" checked={checked} disabled={disabled} onChange={e=>setChecked(e.target.checked)}/>我已核对已保存实验，仍需新建这一份</label>}
    <div className="mixture-study-actions"><button className="mixture-study-primary" type="submit" disabled={disabled||(unconfirmed&&!checked)}>{disabled?"计算中…":"保存并运行条件实验"}</button></div></form>
  </section>;
}
