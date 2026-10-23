import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "../../shared/bridge";

/** 内置和导入皮肤共用同一预览准入；隐藏、系统暂停和减少动画均停止播放。 */
export function useAnimationPreviewEnvironment(reducedMotion = false) {
  const [visible, setVisible] = useState(!document.hidden);
  const [nativeVisible, setNativeVisible] = useState(true);
  const [runtimePaused, setRuntimePaused] = useState(isDesktop);
  const [systemReduced, setSystemReduced] = useState(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  useEffect(() => {
    let live = true, runtimeRevision = 0, visibilityRevision = 0;
    const change = () => setVisible(!document.hidden);
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const preference = () => setSystemReduced(media.matches);
    const nativeVisibility = (event: Event) => {
      visibilityRevision++;
      const detail = (event as CustomEvent).detail;
      setNativeVisible(typeof detail === "boolean" ? detail : detail?.visible !== false);
    };
    const runtime = (event: Event) => {
      runtimeRevision++;
      setRuntimePaused((event as CustomEvent).detail?.paused === true);
    };
    document.addEventListener("visibilitychange", change);
    media.addEventListener("change", preference);
    window.addEventListener("view-visibility", nativeVisibility);
    window.addEventListener("desktop-runtime-state", runtime);
    if (isDesktop) {
      const requestedRuntime = runtimeRevision, requestedVisibility = visibilityRevision;
      void invoke<{ runtime?: { paused?: boolean }; windows?: { main?: { visible?: boolean } } }>("desktop_metrics").then((value) => {
        if (!live) return;
        if (requestedRuntime === runtimeRevision) setRuntimePaused(value.runtime?.paused !== false);
        if (requestedVisibility === visibilityRevision) setNativeVisible(value.windows?.main?.visible !== false);
      }).catch(() => {});
    }
    return () => {
      live = false;
      document.removeEventListener("visibilitychange", change);
      media.removeEventListener("change", preference);
      window.removeEventListener("view-visibility", nativeVisibility);
      window.removeEventListener("desktop-runtime-state", runtime);
    };
  }, []);
  return { visible: visible && nativeVisible, runtimePaused, staticOnly: reducedMotion || systemReduced };
}
