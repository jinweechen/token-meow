import { useCallback, useEffect, useRef, useState } from "react";
import { parseVrmModel, parseVrmState, type VrmSkinClient, type VrmSkinModel, type VrmSkinState } from "../../shared/vrmSkin";

const errorText = (error: unknown) => typeof error === "string" ? error : error instanceof Error ? error.message : "VRM 操作未完成。";
export function useVrmSkins(client: VrmSkinClient, enabled: boolean) {
  const [library, setLibrary] = useState<VrmSkinState | null>(null);
  const [preview, setPreview] = useState<VrmSkinModel | null>(null);
  const [previewRevision, setPreviewRevision] = useState(0);
  const [phase, setPhase] = useState("idle");
  const [error, setError] = useState("");
  const [revision, setRevision] = useState(0);
  const generation = useRef(0), reads = useRef(0), busy = useRef(false);
  useEffect(() => {
    const owner = ++generation.current;
    let live = true, remove: (() => void) | undefined;
    setLibrary(null); setPreview(null); setError(""); setPhase("idle"); busy.current = false;
    if (!enabled) return () => { live = false; ++generation.current; };
    const refresh = async () => {
      const read = ++reads.current;
      try { const state = parseVrmState(await client.state()); if (live && owner === generation.current && read === reads.current) setLibrary(state); }
      catch (error) { if (live && owner === generation.current && read === reads.current) setError(errorText(error)); }
    };
    void client.subscribe(kind => {
      if (!live) return;
      if (kind === "selected") setRevision(value => value + 1);
      void refresh();
    }).then(unsubscribe => { if (!live) unsubscribe(); else { remove = unsubscribe; void refresh(); } })
      .catch(error => { if (live) setError(errorText(error)); });
    return () => { live = false; ++generation.current; ++reads.current; remove?.(); };
  }, [client, enabled]);
  const run = async <T,>(next: string, work: () => Promise<T>, apply: (result: T) => void) => {
    if (!enabled || busy.current) return;
    const owner = generation.current;
    busy.current = true; setPhase(next); setError("");
    try { const result = await work(); if (owner === generation.current) apply(result); }
    catch (error) { if (owner === generation.current) setError(errorText(error)); }
    finally { if (owner === generation.current) { busy.current = false; setPhase("idle"); } }
  };
  const selected = enabled ? library?.models.find(model => model.modelId === library.selectedModelId) ?? null : null;
  const [failure, setFailure] = useState<{ id: string; revision: number; message: string } | null>(null);
  const onFailure = useCallback((message: string) => { if (selected) setFailure({ id: selected.modelId, revision, message }); }, [selected?.modelId, revision]);
  const runtimeError = failure?.id === selected?.modelId && failure?.revision === revision ? failure.message : null;
  return { library, preview, previewRevision, selected, phase, error, revision, runtimeError, onFailure,
    active: runtimeError ? null : selected,
    previewModel(model: VrmSkinModel) { if (!busy.current) { setPreview(model); setPreviewRevision(value => value + 1); setError(""); } },
    importModel: () => run("importing", () => client.chooseImport(), model => {
      if (model) { const value = parseVrmModel(model); setPreview(value); setPreviewRevision(value => value + 1); setLibrary(state => state ? { ...state, models: [...state.models.filter(item => item.modelId !== value.modelId), value] } : null); }
    }),
    select: (modelId: string | null) => run("selecting", () => client.select(modelId), state => { ++reads.current; setLibrary(parseVrmState(state)); setRevision(value => value + 1); }),
    remove: (modelId: string) => run("removing", () => client.remove(modelId), state => { ++reads.current; setLibrary(parseVrmState(state)); setPreview(value => value?.modelId === modelId ? null : value); }),
  };
}
export type VrmSkinsModel = ReturnType<typeof useVrmSkins>;
