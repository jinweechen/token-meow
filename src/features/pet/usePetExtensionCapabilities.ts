import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "../../shared/bridge";

// Native acceptance is separate from implementation/type checks. Development
// owners opt in via their private process environment; ordinary starts stay off.
export function usePetExtensionCapabilities() {
  const [enabled, setEnabled] = useState(false);
  const [roaming, setRoaming] = useState(false);
  useEffect(() => {
    if (!isDesktop) return;
    let live = true;
    void invoke<{ enabled: boolean; roaming?: boolean; nativeQualified: boolean }>("get_pet_extension_capabilities")
      .then(value => { if (live) { setEnabled(value.enabled === true); setRoaming(value.enabled === true && value.roaming === true); } }).catch(() => {});
    return () => { live = false; };
  }, []);
  return { enabled, roaming, nativeQualified: false };
}
