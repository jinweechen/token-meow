import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createLocalPetSkinClient } from "../../shared/localPetSkin";

export const localPetSkinClient = createLocalPetSkinClient(invoke,
  changed => listen<{ kind?: unknown }>("pet-skin-library-changed", event => {
    const kind = event.payload?.kind;
    changed(kind === "selected" || kind === "imported" || kind === "removed" ? { kind } : undefined);
  }));
