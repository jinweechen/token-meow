import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { parseVrmModel, parseVrmState, type VrmSkinClient } from "../../shared/vrmSkin";

export const vrmSkinClient: VrmSkinClient = {
  state: async () => parseVrmState(await invoke("get_vrm_skin_state")),
  async chooseImport() { const value = await invoke("choose_import_vrm_skin"); return value === null ? null : parseVrmModel(value); },
  select: async modelId => parseVrmState(await invoke("select_vrm_skin", { modelId })),
  remove: async modelId => parseVrmState(await invoke("remove_vrm_skin", { modelId })),
  async readModel(modelId) {
    const bytes = await invoke<ArrayBuffer | number[]>("read_vrm_skin_model", { modelId });
    const result = bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes).buffer;
    if (result.byteLength < 32 || result.byteLength > 64 * 1024 * 1024) throw Error("VRM 模型大小无效。");
    return result;
  },
  subscribe: changed => listen<{ kind?: string }>("pet-skin-library-changed", event => changed(event.payload?.kind)),
};
