import { Component, lazy, Suspense, useState, type ReactNode } from "react";
import type { PetState } from "../../shared/contracts";
import type { VrmSkinClient } from "../../shared/vrmSkin";
import type { VrmSkinsModel } from "./useVrmSkins";
import { useAnimationPreviewEnvironment } from "./useAnimationPreviewEnvironment";
import "./vrmSkin.css";

const Avatar = lazy(() => import("./VrmAvatar").then(module => ({ default: module.VrmAvatar })));
class PreviewBoundary extends Component<{ children: ReactNode; failed(message: string): void }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  componentDidCatch() { this.props.failed("VRM 预览暂不可用，请重新加载。"); }
  render() { return this.state.failed ? null : this.props.children; }
}
const actions: { value: PetState; label: string }[] = [
  { value: "idle", label: "待机" }, { value: "active", label: "工作中" }, { value: "complete", label: "完成" },
  { value: "alert", label: "提醒" }, { value: "sleep", label: "休息" },
];
const terms = [
  ["avatarPermission", "角色使用范围"], ["commercialUsage", "商业使用"], ["creditNotation", "署名要求"],
  ["allowRedistribution", "原件再分发"], ["modification", "修改及修改件分发"],
  ["allowExcessivelyViolentUsage", "暴力表达"], ["allowExcessivelySexualUsage", "性表达"],
  ["allowAntisocialOrHateUsage", "反社会或仇恨用途"], ["allowPoliticalOrReligiousUsage", "政治或宗教用途"],
  ["copyrightInformation", "版权说明"], ["licenseUrl", "模型许可"],
  ["otherLicenseUrl", "补充许可"], ["thirdPartyLicenses", "第三方素材说明"],
] as const;
// VRM 1.0 对缺省字段有明确限制；原始 meta 保留，展示注明默认规则。
const defaults: Record<string, unknown> = { avatarPermission: "onlyAuthor", commercialUsage: "personalNonProfit", creditNotation: "required",
  allowRedistribution: false, modification: "prohibited", allowExcessivelyViolentUsage: false, allowExcessivelySexualUsage: false,
  allowAntisocialOrHateUsage: false, allowPoliticalOrReligiousUsage: false };
const values: Record<string, string> = { everyone: "任何人", onlyAuthor: "仅作者", onlySeparatelyLicensedPerson: "需单独授权",
  personalNonProfit: "个人非营利", personalProfit: "个人营利", corporation: "包含企业营利", required: "必须显示", unnecessary: "无需强制显示",
  prohibited: "禁止", allowModification: "允许修改，禁止分发修改件", allowModificationRedistribution: "允许修改及分发修改件" };
const termText = (value: unknown) => typeof value === "boolean" ? value ? "允许" : "禁止" : typeof value === "string" ? values[value] ?? value : "未声明";

export function VrmSkinSettings({ model, client, disabled = false, reducedMotion = false }: {
  model: VrmSkinsModel; client: VrmSkinClient; disabled?: boolean; reducedMotion?: boolean;
}) {
  const preview = model.preview;
  const [readyId, setReadyId] = useState<string | null>(null);
  const [failedId, setFailedId] = useState<string | null>(null);
  const [failure, setFailure] = useState("");
  const [retry, setRetry] = useState(0);
  const [action, setAction] = useState<PetState>("idle");
  const environment = useAnimationPreviewEnvironment(reducedMotion);
  const busy = disabled || model.phase !== "idle";
  const previewKey = preview ? `${preview.modelId}:${model.previewRevision}:${retry}` : "";
  const ready = readyId === previewKey && failedId !== previewKey;
  return <section className="card vrm-skins" aria-label="VRM 角色皮肤">
    <div className="section-heading"><h3>VRM 角色皮肤</h3><span className="badge">本地 3D 角色</span></div>
    <p>导入资源内嵌的 VRM 1.0 文件，先查看模型许可和预览，再选择使用。</p>
    <div className="vrm-skin-actions">
      <button type="button" className="button secondary" disabled={busy} onClick={() => void model.importModel()}>导入 VRM 文件</button>
      <button type="button" className="button secondary" disabled={busy} onClick={() => void model.select(null)}>使用内置角色</button>
    </div>
    {model.phase !== "idle" && <p role="status">{model.phase === "importing" ? "请选择 VRM 文件，正在检查资源…" : "正在更新角色库…"}</p>}
    {(model.error || model.runtimeError) && <p role="alert">{model.error || model.runtimeError}</p>}
    <ul className="vrm-skin-list">{model.library?.models.map(item => <li key={item.modelId}>
      <div><strong>{item.name}</strong><small>{item.authors.join("、")}{model.selected?.modelId === item.modelId ? " · 当前选择" : ""}</small></div>
      <button type="button" className="button secondary small" disabled={busy} onClick={() => model.previewModel(item)}>预览</button>
      <button type="button" className="button secondary small" disabled={busy || model.selected?.modelId === item.modelId} onClick={() => void model.remove(item.modelId)}>移除</button>
    </li>)}</ul>
    {model.library?.problems.map((problem, index) => <p role="alert" key={index}>{problem.message}{problem.modelId && problem.modelId !== model.selected?.modelId &&
      <button type="button" className="button secondary small" disabled={busy} onClick={() => void model.remove(problem.modelId!)}>移除损坏模型</button>}</p>)}
    {preview && <div className="vrm-skin-preview">
      <div>
        <div className="vrm-skin-stage">
          {failedId === previewKey ? <p role="alert">{failure}</p> : <PreviewBoundary key={previewKey} failed={message => { setReadyId(null); setFailedId(previewKey); setFailure(message); }}><Suspense fallback={<span role="status">正在加载角色预览…</span>}>
            <Avatar key={previewKey} model={preview} readModel={client.readModel} action={action}
              reducedMotion={environment.staticOnly} paused={!environment.visible || environment.runtimePaused}
              onReady={id => { if (id === preview.modelId) setReadyId(previewKey); }}
              onFailure={message => { setReadyId(null); setFailedId(previewKey); setFailure(message); }} />
          </Suspense></PreviewBoundary>}
        </div>
        <label>预览状态<select aria-label="VRM 预览状态" value={action} onChange={event => setAction(event.target.value as PetState)}>
          {actions.map(item => <option key={item.value} value={item.value}>{item.label}</option>)}
        </select></label>
        {failedId === previewKey && <button type="button" className="button secondary small" disabled={!environment.visible || environment.runtimePaused}
          onClick={() => { setReadyId(null); setRetry(value => value + 1); }}>重新加载预览</button>}
        <p role="status">{environment.staticOnly ? "减少动画已开启，仅显示静态姿态。" : !ready ? "首帧加载完成后可以使用。" : "预览已就绪，确认后切换角色。"}</p>
        <button type="button" className="button primary" disabled={busy || !ready} onClick={() => void model.select(preview.modelId)}>使用此 VRM 角色</button>
      </div>
      <div><h4>{preview.name}</h4><p>作者：{preview.authors.join("、")}</p>
        <dl>{terms.filter(([key]) => preview.meta[key] !== undefined || key in defaults).map(([key, label]) => <div key={key}><dt>{label}</dt><dd>{termText(preview.meta[key] ?? defaults[key])}{preview.meta[key] === undefined ? "（VRM 1.0 默认）" : ""}</dd></div>)}</dl>
        <p>使用权限由模型作者声明。框架的开源许可不替代模型许可。</p>
      </div>
    </div>}
    <p className="subtle-note">可使用 <a href="https://github.com/pixiv/three-vrm/tree/v3.5.5/packages/three-vrm/examples/models" target="_blank" rel="noreferrer">官方示例资源</a>验证，下载前查看各模型许可。VRM 角色暂使用矩形拖动区域，自动走动暂不启用。</p>
  </section>;
}
