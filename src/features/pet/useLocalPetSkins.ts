import { useEffect, useState } from "react";
import type { LocalPetSkinClient, LocalPetSkinPackage } from "../../shared/localPetSkin";
import { LocalPetSkinController, type LocalPetSkinsSnapshot } from "./localPetSkinController";
import { createLocalPetSkinRuntime, type LocalPetSkinRuntime } from "./localPetSkinRuntime";

export interface LocalPetSkinsModel extends LocalPetSkinsSnapshot {
  refresh(): Promise<void>;
  importSkin(): Promise<void>;
  previewSkin(skinId: string): Promise<void>;
  useSkin(skinId?: string): Promise<boolean | void>;
  useBuiltin(): Promise<boolean | void>;
  removeSkin(skinId: string): Promise<boolean | void>;
  runtimeError?: string | null;
}
const empty: LocalPetSkinsSnapshot = { library: null, selected: null, preview: null,
  phase: "idle", progress: null, previewReady: false, selectionRevision: 0, error: null };
const noop = async () => {};

export function useLocalPetSkins(client: LocalPetSkinClient, enabled = true): LocalPetSkinsModel {
  const [owner, setOwner] = useState<LocalPetSkinController | null>(null);
  const [snapshot, setSnapshot] = useState(empty);
  useEffect(() => {
    if (!enabled) { setOwner(null); setSnapshot(empty); return; }
    const current = new LocalPetSkinController(client);
    const unlisten = current.subscribe(() => setSnapshot(current.snapshot()));
    setOwner(current); setSnapshot(current.snapshot());
    void current.start();
    return () => { unlisten(); current.dispose(); };
  }, [client, enabled]);
  return { ...snapshot, refresh: owner?.refresh ?? noop, importSkin: owner?.importSkin ?? noop,
    previewSkin: owner?.previewSkin ?? noop, useSkin: owner?.useSkin ?? noop, useBuiltin: owner?.useBuiltin ?? noop,
    removeSkin: owner?.removeSkin ?? noop };
}

export function useLocalPetSkinRuntime(pkg: LocalPetSkinPackage | null, client: LocalPetSkinClient,
  enabled = true, onFailure: () => void = noop): LocalPetSkinRuntime | null {
  const [runtime, setRuntime] = useState<LocalPetSkinRuntime | null>(null);
  useEffect(() => {
    if (!enabled || !pkg) { setRuntime(null); return; }
    const current = createLocalPetSkinRuntime(pkg, client, onFailure);
    setRuntime(current);
    return () => current.dispose();
  }, [pkg?.skinId, client, enabled, onFailure]);
  return enabled && pkg?.skinId === runtime?.skinId ? runtime : null;
}

/** Each WebView falls back independently; a decode failure never rewrites selection.json. */
export function useCurrentPetSkinRuntime(client: LocalPetSkinClient, enabled = true) {
  const model = useLocalPetSkins(client, enabled);
  const [failure, setFailure] = useState<{ skinId: string; message: string } | null>(null);
  const [retry, setRetry] = useState(0);
  const [runtime, setRuntime] = useState<LocalPetSkinRuntime | null>(null);
  const skinId = model.selected?.skinId ?? null;
  useEffect(() => { setFailure(null); }, [skinId, model.selectionRevision]);
  useEffect(() => {
    if (!enabled || !model.selected || failure?.skinId === skinId) { setRuntime(null); return; }
    const current = createLocalPetSkinRuntime(model.selected, client, () => {
      setFailure({ skinId: current.skinId, message: "本地皮肤加载失败，当前窗口已恢复内置角色。可预览后重新使用。" });
    });
    setRuntime(current);
    return () => current.dispose();
  }, [enabled, skinId, client, failure?.skinId, retry]);
  return { ...model,
    runtime: enabled && runtime?.skinId === skinId && failure?.skinId !== skinId ? runtime : null,
    runtimeError: failure?.skinId === skinId ? failure.message : null,
    async useSkin(id?: string) {
      const selected = await model.useSkin(id);
      const wanted = id ?? model.preview?.skinId;
      // A failed select does not qualify a retry. The preview must be checked.
      if (selected && wanted && model.previewReady && model.preview?.skinId === wanted) {
        setFailure(null); setRetry((value) => value + 1);
      }
    },
  };
}
