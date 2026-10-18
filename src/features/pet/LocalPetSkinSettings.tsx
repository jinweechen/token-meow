import { useEffect, useState } from "react";
import type { LocalPetSkinClient } from "../../shared/localPetSkin";
import type { LocalPetSkinsModel } from "./useLocalPetSkins";
import { createLocalPetSkinRuntime } from "./localPetSkinRuntime";
import "./localPetSkin.css";

export function LocalPetSkinSettings({ model, client, disabled = false }: {
  model: LocalPetSkinsModel; client: LocalPetSkinClient; disabled?: boolean;
}) {
  const [url, setUrl] = useState("");
  const [previewError, setPreviewError] = useState<string | null>(null);
  const pkg = model.preview;
  const busy = disabled || model.phase !== "idle";
  useEffect(() => {
    setUrl(""); setPreviewError(null);
    if (!pkg || !model.previewReady) return;
    let live = true;
    const runtime = createLocalPetSkinRuntime(pkg, client);
    const clip = runtime.clips.find((item) => item.id === "idle")!;
    void runtime.getRegistry().load("idle", true).then((asset) => {
      if (live && asset) setUrl(asset.frames[clip.fallback].url);
    }).catch(() => { if (live) setPreviewError("预览加载失败，请重新检查素材包"); });
    return () => { live = false; runtime.dispose(); };
  }, [pkg?.skinId, model.previewReady, client]);
  return <section className="card local-pet-skins" aria-label="本地PNG皮肤">
    <div className="section-heading"><h3>本地 PNG 皮肤</h3><span className="badge">目录包 v1</span></div>
    <p>从目录导入 manifest.json 和透明 PNG。导入只预览，点击“使用”后才切换角色。</p>
    <div className="local-skin-actions">
      <button className="button secondary" disabled={busy} onClick={() => { void model.importSkin(); }}>导入皮肤目录</button>
      <button className="button secondary" disabled={busy || !model.selected} onClick={() => { void model.useBuiltin(); }}>使用内置角色</button>
    </div>
    {model.phase === "loading" && <p role="status">正在读取本地皮肤…</p>}
    {model.phase === "importing" && <p role="status">请选择素材包目录，导入后将检查每张图片…</p>}
    {model.phase === "validating" && <p role="status">检查图片 {model.progress?.checked ?? 0} / {model.progress?.total ?? 0}…</p>}
    {model.phase === "selecting" && <p role="status">正在切换角色…</p>}
    {(model.error || model.runtimeError || previewError) && <p className="local-skin-error" role="alert">{model.error || model.runtimeError || previewError}</p>}
    <ul className="local-skin-list">{model.library?.skins.map((skin) => <li key={skin.skinId}>
      <div><strong>{skin.manifest.name}</strong><small>{skin.manifest.author}{model.selected?.skinId === skin.skinId ? " · 当前选择" : ""}</small></div>
      <button className="button secondary small" disabled={busy} onClick={() => { void model.previewSkin(skin.skinId); }}>预览</button>
      <button className="button secondary small" disabled={busy || model.selected?.skinId === skin.skinId}
        onClick={() => { void model.removeSkin(skin.skinId); }}>移除</button>
    </li>)}</ul>
    {!!model.library?.problems?.length && <ul className="local-skin-list">{model.library.problems.map((problem, index) =>
      <li key={`${problem.skinId ?? "store"}-${index}`}><div><strong>皮肤存储诊断</strong><small>{problem.message}</small></div>
        {problem.skinId && <button className="button secondary small" disabled={busy || model.selected?.skinId === problem.skinId}
          onClick={() => { void model.removeSkin(problem.skinId!); }}>移除损坏包</button>}</li>)}</ul>}
    {pkg && <div className="local-skin-preview">
      {url && <img src={url} alt={`${pkg.manifest.name} 静态预览`} onError={() => { setUrl(""); setPreviewError("预览图片无法显示，请重新检查素材包"); }} />}
      <div><h4>{pkg.manifest.name}</h4><dl>
        <dt>作者</dt><dd>{pkg.manifest.author}</dd><dt>来源</dt><dd>{pkg.manifest.source}</dd>
        <dt>许可</dt><dd>{pkg.manifest.license}</dd><dt>署名</dt><dd>{pkg.manifest.attribution}</dd>
      </dl><p>缺少的扩展姿势会使用待机画面；本地皮肤不启用行走移动。</p>
      <button className="button primary" disabled={busy || !model.previewReady || !!previewError || !url}
        onClick={() => { void model.useSkin(pkg.skinId); }}>使用此皮肤</button></div>
    </div>}
  </section>;
}
