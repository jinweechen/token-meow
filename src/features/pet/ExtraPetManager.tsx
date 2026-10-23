import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { extraPetClient, type ExtraPet, type ExtraSkin } from "../../shared/extraPets";
import type { LocalPetSkinPackage } from "../../shared/localPetSkin";
import type { VrmSkinModel } from "../../shared/vrmSkin";

export function ExtraPetManager({ enabled, png = [], vrm = [] }: { enabled: boolean; png?: readonly LocalPetSkinPackage[]; vrm?: readonly VrmSkinModel[] }) {
  const [pets, setPets] = useState<ExtraPet[]>([]), [selected, setSelected] = useState("builtin"), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const live = useRef(false), working = useRef(false), revision = useRef(0), lifecycle = useRef(0);
  useEffect(() => {
    let active = true, remove: (() => void) | undefined;
    ++lifecycle.current; live.current = true; working.current = false; setBusy(false); setPets([]);
    if (!enabled) return () => { live.current = false; };
    const refresh = async () => { const request = ++revision.current; try { const values = await extraPetClient.list(); if (active && request === revision.current) setPets(values); } catch (error) { if (active && request === revision.current) setError(String(error)); } };
    void listen("extra-pets-changed", () => { if (active) void refresh(); }).then(unsubscribe => { if (!active) unsubscribe(); else { remove = unsubscribe; void refresh(); } }).catch(error => { if (active) setError(String(error)); });
    return () => { active = false; live.current = false; ++lifecycle.current; ++revision.current; remove?.(); };
  }, [enabled]);
  const perform = async (work: () => Promise<unknown>) => {
    if (!enabled || working.current) return;
    const owner = lifecycle.current;
    working.current = true; setBusy(true); setError("");
    try { await work(); const request = ++revision.current; const values = await extraPetClient.list(); if (live.current && owner === lifecycle.current && request === revision.current) setPets(values); }
    catch (error) { if (live.current && owner === lifecycle.current) setError(typeof error === "string" ? error : error instanceof Error ? error.message : "辅助桌宠操作未完成。"); }
    finally { if (owner === lifecycle.current) { working.current = false; if (live.current) setBusy(false); } }
  };
  const skin: ExtraSkin = selected === "builtin" ? { kind: "builtin" } : { kind: selected.startsWith("png:") ? "png" : "vrm", id: selected.slice(4) };
  return <section className="card"><div className="section-heading"><h3>辅助桌宠</h3><span className="badge">最多两只</span></div>
    <p>增加一个独立小窗口，单独选择角色、缩放和拖动。共享只读活动摘要，由主桌宠负责业务提醒；退出应用后不自动重建辅助窗口。</p>
    <label>辅助角色<select aria-label="辅助桌宠角色" disabled={!enabled || busy} value={selected} onChange={event => setSelected(event.target.value)}>
      <option value="builtin">内置小柚</option>{png.map(model => <option key={model.skinId} value={`png:${model.skinId}`}>{model.manifest.name} · PNG</option>)}
      {vrm.map(model => <option key={model.modelId} value={`vrm:${model.modelId}`}>{model.name} · VRM</option>)}
    </select></label>
    <button type="button" className="button secondary" disabled={!enabled || busy || pets.length >= 1} onClick={() => void perform(() => extraPetClient.add(skin))}>增加辅助桌宠</button>
    {pets.map(pet => <div key={pet.petId} className="setting-row"><div><strong>辅助桌宠</strong><p>{pet.visible ? "窗口显示已启用" : "窗口已隐藏"} · {Math.round(pet.scale * 100)}%</p></div>
      <button type="button" className="button secondary small" disabled={busy} onClick={() => void perform(() => extraPetClient.configure(pet, { skinChoice: skin }))}>使用所选角色</button>
      <select aria-label="辅助桌宠缩放" value={pet.scale} disabled={busy} onChange={event => void perform(() => extraPetClient.configure(pet, { scale: Number(event.target.value) }))}>
        {[.5,.75,1,1.25,1.5].map(scale => <option key={scale} value={scale}>{scale * 100}%</option>)}
      </select>
      <button type="button" className="button secondary small" disabled={busy} onClick={() => void perform(() => extraPetClient.configure(pet, { visible: !pet.visible }))}>{pet.visible ? "隐藏" : "显示"}</button>
      <button type="button" className="button secondary small" disabled={busy} onClick={() => void perform(() => extraPetClient.remove(pet))}>关闭辅助桌宠</button>
    </div>)}
    {error && <p role="alert">{error}</p>}
  </section>;
}
