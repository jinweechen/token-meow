import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Settings } from "../../shared/contracts";
import * as bridge from "../../shared/bridge";
import { demoPetDetails, demoPetPulse } from "../../shared/demo";
import { PetReadLifecycle, type PetReadState } from "./petLifecycle";

function safeError(error: unknown) {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "本地操作未完成，请查看数据健康";
}

/** A pet window never creates a panel Query or requests the complete ledger. */
export function usePetStore() {
  const demoSettings = useRef<Settings>({});
  const lifecycle = useRef<PetReadLifecycle | null>(null);
  const [state, setState] = useState<PetReadState>({
    pulse: null,
    ready: false,
    details: null,
    detailsReady: false,
    connectionError: null,
    detailsError: null,
  });
  const [pending, setPending] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [nativeError, setNativeError] = useState<string | null>(null);
  const visible = useRef(false);
  const notify = useCallback((text: string) => {
    if (visible.current) setToast(text);
  }, []);
  useEffect(() => {
    if (!toast) return;
    const timeout = window.setTimeout(() => setToast(null), 5500);
    return () => window.clearTimeout(timeout);
  }, [toast]);
  useEffect(() => {
    // StrictMode replays setup/cleanup. Each setup owns a fresh live controller.
    const owner = new PetReadLifecycle({
      now: () => Date.now(),
      pulse: async (cursor) => {
        if (bridge.isDesktop) return bridge.getPetPulse(cursor);
        const pulse = demoPetPulse(cursor);
        return { ...pulse, settings: { ...pulse.settings, ...demoSettings.current } };
      },
      details: async (request) =>
        bridge.isDesktop ? bridge.getPetDetails(request) : demoPetDetails(request),
    });
    lifecycle.current = owner;
    setState(owner.state());
    const unsubscribe = owner.subscribe(setState);
    let live = true;
    let nativeVisible = !bridge.isDesktop;
    let nativePaused = false;
    let firstFrame = 0;
    let secondFrame = 0;
    const removers: (() => void)[] = [];
    const applyVisibility = () => {
      const active = !document.hidden && nativeVisible && !nativePaused;
      visible.current = active;
      if (!active) setToast(null);
      owner.setVisible(active);
    };
    const documentVisibility = () => applyVisibility();
    document.addEventListener("visibilitychange", documentVisibility);
    applyVisibility();
    if (bridge.isDesktop) {
      const names = [
        "ledger-updated",
        "pet-options-changed",
        "account-updated",
        "pet-visibility-changed",
        "desktop-runtime-state",
        "window-visibility-changed",
      ];
      const subscriptions = names.map(async (name) => {
        const remove = await listen(name, (event) => {
          if (!live) return;
          if (name === "pet-visibility-changed") {
            nativeVisible = (event.payload as { visible?: boolean }).visible !== false;
            applyVisibility();
            window.dispatchEvent(new CustomEvent("pet-visibility", { detail: event.payload }));
          } else if (name === "window-visibility-changed") {
            const detail = event.payload as { label?: string; visible?: boolean };
            if (detail.label === "pet") {
              nativeVisible = detail.visible !== false;
              applyVisibility();
              window.dispatchEvent(new CustomEvent("view-visibility", { detail: { visible: nativeVisible } }));
            }
          } else if (name === "desktop-runtime-state") {
            nativePaused = (event.payload as { paused?: boolean }).paused === true;
            applyVisibility();
            window.dispatchEvent(new CustomEvent("desktop-runtime-state", { detail: event.payload }));
          } else if (name === "ledger-updated" && (event.payload as { petDomainChanged?: boolean })?.petDomainChanged === true) {
            owner.domainChanged();
          } else if (name === "pet-options-changed") {
            owner.settingsChanged();
          } else {
            // Do not null the arrival cursor on every ledger notification.
            void owner.request();
          }
        });
        if (!live) remove();
        else removers.push(remove);
      });
      void Promise.allSettled(subscriptions).then((results) => {
        if (!live) return;
        const failed = results.find((result) => result.status === "rejected");
        if (failed?.status === "rejected") setNativeError(safeError(failed.reason));
        firstFrame = requestAnimationFrame(() => {
          secondFrame = requestAnimationFrame(() => {
            void invoke<{ visible: boolean; paused: boolean }>("frontend_ready")
              .then((initial) => {
                if (!live) return;
                nativeVisible = initial.visible;
                nativePaused = initial.paused;
                applyVisibility();
                window.dispatchEvent(new CustomEvent("initial-view-state", { detail: initial }));
                window.dispatchEvent(new CustomEvent("view-visibility", { detail: { visible: nativeVisible } }));
                window.dispatchEvent(new CustomEvent("desktop-runtime-state", { detail: { paused: nativePaused } }));
              })
              .catch((error) => { if (live) setNativeError(safeError(error)); });
          });
        });
      });
    }
    return () => {
      live = false;
      visible.current = false;
      cancelAnimationFrame(firstFrame);
      cancelAnimationFrame(secondFrame);
      document.removeEventListener("visibilitychange", documentVisibility);
      removers.forEach((remove) => remove());
      unsubscribe();
      owner.dispose();
      if (lifecycle.current === owner) lifecycle.current = null;
    };
  }, []);
  const save = useCallback(async (settings: Settings) => {
    setPending(true);
    try {
      if (bridge.isDesktop) await bridge.runAction("update_settings", { settings });
      else {
        demoSettings.current = { ...demoSettings.current, ...settings };
        if (settings.doNotDisturb !== undefined)
          demoSettings.current.effectiveDoNotDisturb = settings.doNotDisturb;
      }
      await lifecycle.current?.request();
      notify(bridge.isDesktop ? "偏好已保存" : "演示偏好已更新；不会保存到桌面应用");
    } catch (error) {
      notify(safeError(error));
    } finally {
      setPending(false);
    }
  }, [notify]);
  const resetPet = useCallback(async () => {
    if (!bridge.isDesktop) {
      notify("一键找回在 Windows 桌面应用中可用");
      return;
    }
    try {
      await bridge.resetPetPosition();
      await lifecycle.current?.request();
      notify("小柚已回到可见工作区");
    } catch (error) { notify(safeError(error)); }
  }, [notify]);
  const setDetailsOpen = useCallback((open: boolean) => lifecycle.current?.setDetailsOpen(open), []);
  return {
    ...state,
    presentation: state.pulse,
    pending,
    toast,
    connectionError: state.connectionError ?? nativeError,
    demo: !bridge.isDesktop,
    setDetailsOpen,
    refresh: () => lifecycle.current?.request() ?? Promise.resolve(),
    notify,
    save,
    resetPet,
  };
}
