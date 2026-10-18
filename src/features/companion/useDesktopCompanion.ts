import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as bridge from "../../shared/bridge";
import { CompanionStateReader, validCompanionState, type DesktopCompanionState, type CompanionLauncher } from "../../shared/desktopCompanion";
import { CompanionMotionPresentation } from "./motionPresentation";

export function companionError(error: unknown) {
  return typeof error === "string" ? error : error instanceof Error ? error.message : "桌面联动暂未完成";
}
export function useDesktopCompanion() {
  const [state, setState] = useState<DesktopCompanionState | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!bridge.isDesktop) return;
    const reader = new CompanionStateReader();
    const motion = new CompanionMotionPresentation();
    setState(null);
    let expiry: number | undefined;
    let live = true, running = false, again = false;
    let remove: (() => void) | undefined;
    const publish = () => {
      if (!live) return;
      if (expiry !== undefined) { window.clearTimeout(expiry); expiry = undefined; }
      const value = reader.state();
      if (!value) { setState(null); return; }
      const now = performance.now();
      motion.observe(value, now);
      const lease = motion.read(now);
      const active = value.phase === "playing" || value.phase === "moving";
      const matchingLease = lease?.runtimeEpoch === value.runtimeEpoch &&
        lease.motionGeneration === value.motionGeneration && lease.phase === value.phase ? lease : null;
      setState(active && !matchingLease ? null : value);
      if (active && matchingLease) expiry = window.setTimeout(() => {
        expiry = undefined;
        motion.read(performance.now());
        if (live) publish();
      }, Math.max(1, Math.ceil(matchingLease.remainingMs)));
    };
    const invalidateMotion = () => {
      motion.clear();
      publish();
    };
    const visibilityChanged = () => { if (document.visibilityState !== "visible") invalidateMotion(); };
    document.addEventListener("visibilitychange", visibilityChanged);
    window.addEventListener("pagehide", invalidateMotion);
    const refresh = async () => {
      if (!live) return;
      if (running) { again = true; return; }
      running = true;
      try {
        const value = await bridge.getDesktopCompanionState();
        if (!live) return;
        if (!validCompanionState(value)) throw new Error("桌面活动状态暂未更新");
        if (reader.snapshot(value)) {
          // An event cannot establish or replace the authority domain.
          motion.bindEpoch(reader.state()!.runtimeEpoch, performance.now());
        }
        publish();
        setError(null);
      } catch (cause) { if (live) { invalidateMotion(); setError(companionError(cause)); } }
      finally {
        running = false;
        if (live && again) { again = false; void refresh(); }
      }
    };
    void listen<DesktopCompanionState>("desktop-companion-state", ({ payload }) => {
      if (!live) return;
      if (!validCompanionState(payload)) { invalidateMotion(); setError("桌面活动状态暂未更新"); return; }
      const result = reader.event(payload);
      if (result === "accepted") { publish(); setError(null); }
      else if (result === "refresh") void refresh();
    }).then((unsubscribe) => {
      if (!live) unsubscribe();
      else { remove = unsubscribe; void refresh(); }
    }).catch((cause) => { if (live) { invalidateMotion(); setError(companionError(cause)); } });
    return () => {
      live = false; motion.clear(); if (expiry !== undefined) window.clearTimeout(expiry); remove?.();
      document.removeEventListener("visibilitychange", visibilityChanged);
      window.removeEventListener("pagehide", invalidateMotion);
    };
  }, []);
  return { state, error };
}

/** Requested only while settings/menu is mounted; no polling or executable paths. */
export function useCompanionLaunchers(enabled: boolean) {
  const [launchers, setLaunchers] = useState<CompanionLauncher[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const refreshRef = useRef<(() => Promise<void>) | null>(null);
  const liveRef = useRef(false);
  const busyRef = useRef(false);
  useEffect(() => {
    if (!bridge.isDesktop || !enabled) { setLaunchers([]); return; }
    let live = true, running = false, again = false;
    liveRef.current = true;
    let remove: (() => void) | undefined;
    const refresh = async () => {
      if (!live) return;
      if (running) { again = true; return; }
      running = true;
      try {
        const apps = await bridge.listCompanionLaunchers();
        if (live) { setLaunchers(apps); setError(null); }
      } catch (cause) { if (live) setError(companionError(cause)); }
      finally {
        running = false;
        if (live && again) { again = false; void refresh(); }
      }
    };
    refreshRef.current = refresh;
    void listen("companion-launchers-changed", () => { void refresh(); }).then((unsubscribe) => {
      if (!live) unsubscribe(); else { remove = unsubscribe; void refresh(); }
    }).catch((cause) => { if (live) setError(companionError(cause)); });
    return () => {
      live = false; liveRef.current = false;
      if (refreshRef.current === refresh) refreshRef.current = null;
      remove?.();
    };
  }, [enabled]);
  const perform = useCallback(async (operation: () => Promise<unknown>) => {
    if (!bridge.isDesktop || !enabled || busyRef.current) return false;
    busyRef.current = true;
    setPending(true);
    try {
      const result = await operation();
      if (result && typeof result === "object" && "accepted" in result && result.accepted !== true)
        throw new Error("应用未接受这次打开请求");
      await refreshRef.current?.();
      return true;
    } catch (cause) {
      if (liveRef.current) setError(companionError(cause));
      return false;
    } finally {
      busyRef.current = false;
      if (liveRef.current) setPending(false);
    }
  }, [enabled]);
  return { launchers, pending, error, perform };
}
