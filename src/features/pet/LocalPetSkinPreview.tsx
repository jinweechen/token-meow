import { useEffect, useState } from "react";
import type { LocalPetSkinClient, LocalPetSkinPackage } from "../../shared/localPetSkin";
import { createLocalPetSkinRuntime, type LocalPetSkinRuntime } from "./localPetSkinRuntime";
import { useAnimationClip } from "./useAnimationClip";
import { useAnimationPreviewEnvironment } from "./useAnimationPreviewEnvironment";

const labels: Record<string, string> = { idle: "待机", active: "工作中", complete: "完成", alert: "提醒", sleep: "休息",
  shy: "害羞", affection: "摸摸回应", stretch: "伸懒腰", wink: "眨眼", happy: "开心", bow: "鞠躬", "hug-tail": "抱尾巴", walk: "原地侧走" };

export function LocalPetSkinPreview({ pkg, client, reducedMotion, disabled, onUse }: {
  pkg: LocalPetSkinPackage; client: LocalPetSkinClient; reducedMotion: boolean; disabled: boolean; onUse(): void;
}) {
  const [runtime, setRuntime] = useState<LocalPetSkinRuntime | null>(null);
  const [clip, setClip] = useState("idle");
  const [playing, setPlaying] = useState(false);
  const [generation, setGeneration] = useState(0);
  const [failed, setFailed] = useState(false);
  const [painted, setPainted] = useState<string | null>(null);
  const { visible, runtimePaused, staticOnly } = useAnimationPreviewEnvironment(reducedMotion);
  useEffect(() => {
    const owner = createLocalPetSkinRuntime(pkg, client, () => setFailed(true));
    setRuntime(owner);
    return () => owner.dispose();
  }, [pkg, client]);
  const sprite = useAnimationClip(clip, playing && !staticOnly, !!runtime && visible && !runtimePaused && !failed,
    `local-preview:${pkg.skinId}:${generation}`, true, runtime);
  const assetReady = !!sprite?.current && !!sprite.url && !sprite.error && !failed;
  const loaded = assetReady && painted === sprite?.url;
  useEffect(() => {
    if (!visible || runtimePaused || staticOnly) setPlaying(false);
  }, [visible, runtimePaused, staticOnly]);
  const change = (value: string) => { setClip(value); setFailed(false); setPlaying(false); setGeneration((n) => n + 1); };
  return <div className="local-skin-player">
    <div className="local-skin-preview-stage" data-local-preview-clip={clip}>
      {assetReady && sprite ? <img src={sprite.url} alt={`${pkg.manifest.name}：${labels[clip] ?? clip}`} draggable={false}
        data-animation-frame={sprite.frame} onLoad={() => setPainted(sprite.url)} onError={() => { setFailed(true); runtime?.cancel(); }} />
        : <span role="status">{failed || sprite?.error ? "预览加载失败，可重新加载" : "正在加载预览…"}</span>}
    </div>
    <label>预览动作<select aria-label="本地皮肤预览动作" value={clip} onChange={(event) => change(event.target.value)}>
      {pkg.manifest.clips.map((item) => <option key={item.id} value={item.id}>{labels[item.id] ?? item.id}</option>)}
    </select></label>
    <div className="local-skin-actions">
      <button type="button" className="button secondary small" disabled={staticOnly || runtimePaused || !visible}
        onClick={() => { setFailed(false); setPlaying(true); setGeneration((n) => n + 1); }}>播放一次</button>
      <button type="button" className="button secondary small" disabled={!playing} onClick={() => setPlaying(false)}>静态预览</button>
      {(failed || sprite?.error) && <button type="button" className="button secondary small" disabled={runtimePaused || !visible}
        onClick={() => { setFailed(false); setPlaying(false); setGeneration((n) => n + 1); }}>重新加载静态预览</button>}
    </div>
    <p role="status">{staticOnly ? "减少动画已开启，仅显示静态姿态。" : failed ? "当前预览失败，不会切换角色。"
      : playing && sprite?.complete ? "本次播放已结束。" : playing ? "正在播放所选动作。" : "预览不会改变当前角色。"}</p>
    <button className="button primary" disabled={disabled || !loaded} onClick={onUse}>使用此皮肤</button>
  </div>;
}
