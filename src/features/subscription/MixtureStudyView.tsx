import type { MixtureStudiesOverview, MixtureStudy } from "../../shared/mixtureStudyContracts";
import { mixtureStudyReason, mixtureStudyTime } from "../../shared/mixtureStudyPresentation";
import { MixtureStudyCurve, mixtureNumber } from "./MixtureStudyCurve";

export function MixtureStudyReasons({values}:{values:string[]}) {return values.length?<ul className="mixture-study-reasons">{[...new Set(values)].map(v=><li key={v}>{mixtureStudyReason(v)}</li>)}</ul>:null;}
export function MixtureStudyView({overview,disabled,editingId,onEdit,onDelete}:{overview:MixtureStudiesOverview;disabled:boolean;editingId:string|null;onEdit:(value:MixtureStudy)=>void;onDelete:(value:MixtureStudy)=>Promise<boolean>}) {
  return <div className="mixture-study-list">{!overview.studies.length&&<p className="mixture-study-empty">此来源还没有手动实验。可以填写自己的条件，或显式填入合成示例后检查、保存。</p>}
    {overview.studies.map(study=>{const fit=study.projection.fit,c=study.config;return <article className="mixture-study-card" key={study.id} aria-label={`${c.title} 手动实验`}>
      <div className="mixture-study-heading"><div><h3>{c.title}</h3><span className="mixture-study-tag">全手动条件 · 非官方额度 · 不发送通知</span></div><div className="mixture-study-actions"><button type="button" disabled={disabled} onClick={()=>onEdit(study)}>修改手动实验</button><button type="button" disabled={disabled} onClick={()=>{void onDelete(study);}}>删除手动实验</button></div></div>
      <p className="mixture-study-note">用户条件：{c.condition.accountLabel} / {c.condition.windowLabel} / {c.condition.effort} / {c.condition.speed}</p><p className="mixture-study-note">保存于 {mixtureStudyTime(study.updatedAt)}（上海）· 修订 {study.revision} · 点数为用户假定的手动扣额点，不能换作官方百分点或公开费率。</p>
      {study.bindingState!=="ready"&&<p className="mixture-study-empty">来源绑定已改变或不可用。输入仍保留供检查，需按当前来源范围显式重新保存，旧结果不作当前指导。</p>}
      {editingId===study.id?<p className="mixture-study-empty">正在编辑此实验，旧版本结果暂停展示。保存新输入后由 Core 重新计算。</p>:<>
        <section aria-label="条件模型拟合结果"><div className="mixture-study-heading"><h4>{fit.kind==="separated"?"可区分的条件模型关系":fit.kind==="fixed_mix"?"只能解释固定组合":"暂不发布模型关系"}</h4><span className="mixture-study-tag">条件参考，误差范围未知</span></div>
          <dl className="mixture-study-values"><div><dt>训练样本</dt><dd>{fit.trainSamples}</dd></div><div><dt>固定时间留出</dt><dd>{fit.holdoutSamples}</dd></div><div><dt>矩阵秩 / 模型数</dt><dd>{fit.rank} / {c.models.length}</dd></div><div><dt>条件数</dt><dd>{mixtureNumber(fit.conditionNumber)}</dd></div><div><dt>训练 WAPE</dt><dd>{fit.trainWape===null?"未知":`${mixtureNumber(fit.trainWape*100)}%`}</dd></div><div><dt>留出 WAPE</dt><dd>{fit.holdoutWape===null?"未知":`${mixtureNumber(fit.holdoutWape*100)}%`}</dd></div></dl>
          {fit.kind==="separated"&&fit.weights&&<div className="mixture-study-table-wrap"><table><thead><tr><th>手动模型标签</th><th>每百万 Token 的条件手动扣额点</th></tr></thead><tbody>{fit.weights.map(w=><tr key={w.model}><td>{w.model}</td><td>{mixtureNumber(w.pointsPerMillionTokens)}{w.pointsPerMillionTokens===0&&<small>仅诊断；该模型的正 Token 进入记录保持未知</small>}</td></tr>)}</tbody></table></div>}
          {fit.kind==="fixed_mix"&&fit.fixedMix&&<p className="mixture-study-note">仅限 {c.models.map((m,i)=>`${m} ${mixtureNumber(fit.fixedMix!.shares[i]*100)}%`).join(" / ")} 这一组合，每百万组合 Token 对应 {mixtureNumber(fit.fixedMix.pointsPerMillionTokens)} 手动点。不拆分单模型系数，不用于任意未来换比。</p>}
          <MixtureStudyReasons values={fit.reasons}/><p className="mixture-study-note">模型标签与样本由用户声明。关系仅描述所填条件，不能证明模型质量、厂商容量或官方恢复规则；留出未通过时不保留可用系数。</p>
        </section><MixtureStudyCurve value={study.projection.rolling} config={c}/><MixtureStudyReasons values={[...study.projection.reasons,...study.projection.rolling.reasons]}/>
      </>}
    </article>;})}
  </div>;
}
