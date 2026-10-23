import { useEffect, useState } from "react";
import { AnimationClipRegistry } from "./animationClipRegistry";
import { builtinAnimationClips, builtinAnimationFrames, builtinAnimationSources } from "./builtinAnimationSkin";
import { useAnimationClip } from "./useAnimationClip";
import { useAnimationPreviewEnvironment } from "./useAnimationPreviewEnvironment";
import "./builtinAnimationPreview.css";

const labels: Record<string, string> = {
  idle: "待机", active: "工作中", alert: "提醒", complete: "完成", happy: "开心",
  wink: "眨眼", affection: "摸摸回应", shy: "害羞", bow: "鞠躬", stretch: "伸懒腰",
  "hug-tail": "抱尾巴", sleep: "坐睡", "walk-right": "向右侧走",
  "walk-left": "向左侧走", "walk-left-to-right": "左走转右走", "walk-right-to-left": "右走转左走",
};

function PreviewPlayer({ reducedMotion }: { reducedMotion: boolean }) {
  const [registry, setRegistry] = useState<AnimationClipRegistry | null>(null);
  const [clip, setClip] = useState("idle");
  const [phase, setPhase] = useState<"static" | "playing" | "complete" | "error">("static");
  const [generation, setGeneration] = useState(0);
  const [imageFailed, setImageFailed] = useState(false);
  const { visible, runtimePaused, staticOnly } = useAnimationPreviewEnvironment(reducedMotion);
  useEffect(() => {
    const owner = new AnimationClipRegistry(builtinAnimationFrames, builtinAnimationSources, builtinAnimationClips);
    setRegistry(owner);
    return () => owner.dispose();
  }, []);
  const playing = phase === "playing" || phase === "complete";
  const sprite = useAnimationClip(clip, playing && !staticOnly,
    !!registry && visible && !runtimePaused && !imageFailed && phase !== "error",
    `builtin-preview:${generation}`, true, null, undefined, registry ?? undefined);
  useEffect(() => {
    if (!visible || runtimePaused || staticOnly) setPhase("static");
  }, [visible, runtimePaused, staticOnly]);
  useEffect(() => {
    if (!sprite?.current) return;
    if (sprite.error) setPhase("error");
    else if (phase === "playing" && sprite.complete) setPhase("complete");
  }, [sprite?.current, sprite?.error, sprite?.complete, phase]);
  const replay = () => {
    setImageFailed(false);
    setPhase("playing");
    setGeneration((value) => value + 1);
  };
  return <div className="builtin-animation-player">
    <div className="builtin-animation-controls">
      <label>动作<select aria-label="内置角色动作" value={clip} onChange={(event) => {
        setClip(event.target.value); setImageFailed(false); setPhase("static"); setGeneration((value) => value + 1);
      }}>{builtinAnimationClips.map((item) => <option key={item.id} value={item.id}>{labels[item.id] ?? item.id}</option>)}</select></label>
      <button type="button" className="button secondary" onClick={replay} disabled={staticOnly || runtimePaused}>播放一次</button>
      <button type="button" className="button secondary" onClick={() => setPhase("static")} disabled={!playing}>静态预览</button>
      {(phase === "error" || sprite?.error || imageFailed) && <button type="button" className="button secondary" disabled={!visible || runtimePaused}
        onClick={() => { setImageFailed(false); setPhase("static"); setGeneration((value) => value + 1); }}>重新加载静态预览</button>}
    </div>
    <div className="builtin-animation-stage" data-builtin-preview-clip={clip}>
      {sprite?.current && sprite.url && !sprite.error && !imageFailed
        ? <img src={sprite.url} alt={`小柚：${labels[clip] ?? clip}`} draggable={false}
          data-animation-clip={sprite.clip} data-animation-frame={sprite.frame}
          onError={() => { setImageFailed(true); setPhase("error"); registry?.cancel(); }} />
        : <span role="status">{phase === "error" || sprite?.error || imageFailed ? "预览加载失败，请重新播放" : "正在加载角色"}</span>}
    </div>
    <p role="status">{staticOnly ? "减少动画已开启，仅显示静态姿态。"
      : phase === "complete" ? "本次播放已结束。" : phase === "playing" ? "正在播放完整动作。" : "选择动作后点击播放。"}</p>
    <p>动作在原地展示。侧走和掉头预览不会移动桌宠窗口。</p>
  </div>;
}

export function BuiltinAnimationPreview({ reducedMotion = false }: { reducedMotion?: boolean }) {
  const [open, setOpen] = useState(false);
  return <section className="card builtin-animation-preview">
    <div className="section-heading"><h3>内置角色动作</h3>
      <button type="button" className="button secondary" aria-expanded={open}
        onClick={() => setOpen((value) => !value)}>{open ? "收起动作预览" : "展开动作预览"}</button></div>
    {open && <PreviewPlayer reducedMotion={reducedMotion} />}
  </section>;
}
