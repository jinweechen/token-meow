import { useEffect, useRef, useState, type CSSProperties } from "react";
import { CatGirl } from "./CatGirl";
import { VrmPetCharacter } from "./VrmPetCharacter";
import { createLocalPetSkinRuntime, type LocalPetSkinRuntime } from "./localPetSkinRuntime";
import type { LocalPetSkinClient } from "../../shared/localPetSkin";
import { presentPet } from "./presenter";
import { usePetGesture } from "./usePetGesture";
import { extraPetWindowClient, useExtraPet, type ExtraPetWindowClient } from "./useExtraPet";

/** 独立辅助视图只读取共享活动；不创建采集器、不读取详情、不确认通知。 */
export function ExtraPetApplication({ client = extraPetWindowClient }: { client?: ExtraPetWindowClient }) {
  const model = useExtraPet(client);
  const context = model.context;
  const key = context ? JSON.stringify([context.pet.petId, context.pet.instanceEpoch, context.pet.skinChoice, context.pet.scale, context.pet.visible]) : "pending";
  const [runtime, setRuntime] = useState<{ owner: string; value: LocalPetSkinRuntime } | null>(null);
  const [failure, setFailure] = useState<{ key: string; message: string } | null>(null);
  const [retry, setRetry] = useState(0);
  const resourceKey = `${key}:${retry}:${model.renderer?.token ?? "pending"}`;
  const currentKey = useRef(resourceKey); currentKey.current = resourceKey;
  const [stateText, setStateText] = useState("正在连接辅助桌宠…");
  useEffect(() => {
    document.documentElement.classList.add("pet-mode"); document.body.classList.add("pet-mode");
    return () => { document.documentElement.classList.remove("pet-mode"); document.body.classList.remove("pet-mode"); };
  }, []);
  useEffect(() => {
    setFailure(null); setRuntime(null);
    if (!context?.png || !model.host.visible) return;
    let live = true;
    const denied = async (): Promise<never> => { throw Error("辅助桌宠不提供素材管理权限。"); };
    const pngClient: LocalPetSkinClient = { getState: denied, chooseImport: denied, select: denied, remove: denied, readFrame: client.readFrame };
    let owned: LocalPetSkinRuntime;
    try { owned = createLocalPetSkinRuntime(context.png, pngClient, () => {
      if (live && currentKey.current === resourceKey) setFailure({ key: resourceKey, message: "PNG 皮肤加载失败，本窗口已回退内置角色。" });
    }); } catch { setFailure({ key: resourceKey, message: "PNG 皮肤无法读取，本窗口已回退内置角色。" }); return; }
    setRuntime({ owner: resourceKey, value: owned });
    return () => { live = false; owned.dispose(); };
  }, [resourceKey, client, context?.png, model.host.visible]);
  const intent = presentPet(model.presentation, model.ready);
  const quiet = model.presentation?.settings.effectiveDoNotDisturb ?? model.presentation?.settings.doNotDisturb;
  const affection = () => {
    if (!model.ready || !model.renderer || quiet || model.reducedMotion || intent.action !== "idle" || intent.active > 0 || intent.uncertain > 0) return;
    window.dispatchEvent(new CustomEvent("pet-affection", { detail: { pose: "affection" } }));
  };
  const gesture = usePetGesture({ tap: affection, interrupt: () => window.dispatchEvent(new CustomEvent("pet-interaction")), drag: model.drag });
  const failed = failure?.key === resourceKey;
  const missing = context?.pet.skinChoice.kind === "png" && !context.png || context?.pet.skinChoice.kind === "vrm" && !context.vrm;
  const skinError = failed ? failure.message : missing ? "配置的角色资源不可用，本窗口已回退内置角色。" : "";
  const scale = context?.pet.scale ?? 1;
  return <main className="pet-window pet-stage" aria-label="辅助桌宠" style={{ "--pet-stage-width": `${280 * scale}px`, "--pet-stage-height": `${340 * scale}px` } as CSSProperties}
      onContextMenu={event => event.preventDefault()}>
    <button type="button" className="icon-button" aria-label="关闭辅助桌宠" disabled={!model.renderer}
      title={model.renderer ? "仅关闭当前辅助桌宠" : "请在主面板关闭这个辅助桌宠"}
      style={{ position: "absolute", right: 8, top: 8, zIndex: 4 }} onClick={() => void model.close()}>×</button>
    <button {...gesture()} type="button" className="pet-character-button" aria-label="辅助桌宠，单击摸摸，拖动移动"
      onKeyDown={event => { if (!event.repeat && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); affection(); } }}>
      {context && model.renderer ? context.vrm && !failed ? <VrmPetCharacter key={resourceKey}
          model={context.vrm} readModel={client.readModel} presentation={model.presentation} ready={model.ready}
          hostEnvironment={model.host} onStateText={setStateText}
          onFailure={message => { if (currentKey.current === resourceKey) setFailure({ key: resourceKey, message: `${message}；本窗口已回退内置角色。` }); }} />
        : <CatGirl key={resourceKey} presentation={model.presentation} ready={model.ready} decorative skinRuntime={!failed && context.png && runtime?.owner === resourceKey ? runtime.value : null}
          pointerEnabled={false} roamingEnabled={false} onStateText={setStateText} />
        : <span role="status" style={{ display: "block", lineHeight: 1.5, padding: 24, color: "#557062" }}>正在连接辅助桌宠…</span>}
    </button>
    <div className="pet-status-badge" aria-live="polite"><span className={`status-dot ${intent.uncertain ? "amber" : ""}`} /><span>{model.ready ? stateText : model.error || "活动数据待更新"}</span></div>
    {context?.vrm && context.vrm.meta.creditNotation !== "unnecessary" && !failed && <span className="pet-vrm-credit" title={`${context.vrm.name} · ${context.vrm.authors.join("、")}`}>{context.vrm.authors.join("、")}</span>}
    {(model.error || model.read.connectionError || skinError) && <div className="pet-health-badge" role="status" style={{ maxWidth: "100%", whiteSpace: "normal", textAlign: "center", bottom: 0 }}>
      {model.error || model.read.connectionError || skinError}
      {!model.renderer && <span> 请在主面板恢复或关闭此窗口。</span>}
      <button type="button" className="button secondary small" onClick={() => { setRetry(value => value + 1); model.refresh(); }}>重新读取</button>
    </div>}
  </main>;
}
