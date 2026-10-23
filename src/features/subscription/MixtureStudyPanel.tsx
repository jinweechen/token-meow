import { useEffect, useRef, useState } from "react";
import type { SourceInstance } from "../../shared/contracts";
import { MixtureStudyReadModel, mixtureStudyClient, type MixtureStudyViewState } from "../../shared/mixtureStudyClient";
import type { MixtureStudiesOverview, MixtureStudy, SaveMixtureStudy } from "../../shared/mixtureStudyContracts";
import { sourceKindName } from "../../shared/sourceKind";
import { mixtureStudyTime } from "../../shared/mixtureStudyPresentation";
import { MixtureStudyEditor } from "./MixtureStudyEditor";
import { MixtureStudyReasons, MixtureStudyView } from "./MixtureStudyView";
import "./mixtureStudy.css";

const initial=():MixtureStudyViewState=>({overview:null,pending:false,fresh:false,action:null,error:null,notice:null,formEpoch:0});
export interface MixtureStudyPanelProps {sources:SourceInstance[];scopeKey:string;enabled:boolean;privacyMode:boolean}
export function MixtureStudyPanel({sources,scopeKey,enabled,privacyMode}:MixtureStudyPanelProps) {
  const [selected,setSelected]=useState(()=>sources.find(s=>s.primary&&s.enabled)?.id??sources.find(s=>s.enabled)?.id??"");
  const source=sources.find(s=>s.enabled&&s.id===selected)??sources.find(s=>s.enabled),sourceId=source?.id??"";
  const domain=JSON.stringify([scopeKey,enabled,privacyMode,sourceId,source?.clientKind,source?.root,source?.environment,source?.enabled,source?.configurationRevision]);
  const currentDomain=useRef(domain);currentDomain.current=domain;
  const controller=useRef<{domain:string;model:MixtureStudyReadModel}|null>(null);
  const [view,setView]=useState({domain,value:initial()});
  const [editor,setEditor]=useState<{study:MixtureStudy|null;overview:MixtureStudiesOverview;key:number}|null>(null),key=useRef(0);
  useEffect(()=>{
    setEditor(null);setView({domain,value:initial()});
    if(!enabled||privacyMode||!sourceId)return;
    const model=new MixtureStudyReadModel(mixtureStudyClient,sourceId,value=>{if(currentDomain.current===domain)setView({domain,value});});
    controller.current={domain,model};model.start();
    // 模拟基准冻结，不用墙钟轮询推进情景。恢复可见时只重读保存版本。
    const visible=()=>{if(!document.hidden)model.reload();};document.addEventListener("visibilitychange",visible);
    return()=>{model.dispose();document.removeEventListener("visibilitychange",visible);if(controller.current?.model===model)controller.current=null;};
  },[domain,enabled,privacyMode,sourceId]);
  const state=view.domain===domain&&enabled&&!privacyMode?view.value:initial(),overview=state.overview;
  useEffect(()=>{setEditor(null);},[state.formEpoch]);
  const disabled=!enabled||privacyMode||!overview?.source.enabled||!state.fresh||state.pending||view.domain!==domain;
  const owner=()=>controller.current?.domain===currentDomain.current&&enabled&&!privacyMode?controller.current.model:null;
  async function save(input:SaveMixtureStudy){const model=owner();if(!model)return false;const ok=await model.mutate("save",input.expectedScope,g=>mixtureStudyClient.save(input,g),"手动条件实验已保存并计算；不改变官方额度或通知资格。");if(ok&&currentDomain.current===domain)setEditor(null);return ok;}
  function remove(study:MixtureStudy){const model=owner();if(!model||!overview)return Promise.resolve(false);const input={sourceId,expectedScope:overview.scopeToken,id:study.id,expectedRevision:study.revision};return model.mutate("delete",input.expectedScope,g=>mixtureStudyClient.delete(input,g),"手动实验已删除。");}
  const edit=(study:MixtureStudy|null)=>{if(overview&&!disabled)setEditor({study,overview,key:++key.current});};
  return <section className="mixture-study-panel" aria-labelledby="mixture-study-title"><header className="mixture-study-heading"><div><h2 id="mixture-study-title">条件模型与滑动窗口实验</h2><p className="mixture-study-note">全手动样本拟合与有限窗口模拟。目录不是账号；这里的点数、条件和结果均不代表官方额度，也不产生原生提醒。</p></div><div className="mixture-study-actions"><button type="button" disabled={!enabled||privacyMode||!sourceId||state.pending} onClick={()=>owner()?.reload()}>更新已保存实验</button><button className="mixture-study-primary" type="button" disabled={disabled} onClick={()=>edit(null)}>新建手动实验</button></div></header>
    <label className="mixture-study-source">实验来源<select value={sourceId} disabled={!enabled||privacyMode||!sources.some(s=>s.enabled)} onChange={e=>setSelected(e.target.value)}>{!sourceId&&<option value="">暂无已启用来源</option>}{sources.map((s,i)=><option key={s.id} value={s.id} disabled={!s.enabled}>{sourceKindName(s.clientKind)} · 来源 {i+1}{s.primary?"（官方主来源）":""}{s.enabled?"":"（已停用）"}{privacyMode?"":` · ${s.root}`}</option>)}</select></label>
    {!enabled&&<p className="mixture-study-empty">当前暂停读取手动实验，待来源操作完成后重新载入。</p>}{enabled&&privacyMode&&<p className="mixture-study-empty">隐私模式已隐藏手动条件、样本与曲线，并关闭读写。</p>}{enabled&&!privacyMode&&!sourceId&&<p className="mixture-study-empty">请先登记并启用一个来源。</p>}
    {state.error&&<p className="mixture-study-error" role="alert">{state.error} 请检查更新后的保存版本，失败写入不会自动重发。</p>}{state.notice&&<p className="mixture-study-message" role="status">{state.notice}</p>}{state.pending&&!overview&&<p className="mixture-study-empty">正在读取手动实验…</p>}
    {overview&&<><p className="mixture-study-note">读取于 {mixtureStudyTime(overview.asOf)}（上海）· 保存视图修订 {overview.revision} · 不推进实验冻结时刻{!state.fresh?" · 范围核对中，写入暂不可用":""}</p><MixtureStudyReasons values={overview.reasons}/>{editor&&<MixtureStudyEditor key={editor.key} overview={editor.overview} existing={editor.study} disabled={disabled} onSave={save} onCancel={()=>setEditor(null)}/>}<MixtureStudyView overview={overview} disabled={disabled} editingId={editor?.study?.id??null} onEdit={edit} onDelete={remove}/></>}
  </section>;
}
