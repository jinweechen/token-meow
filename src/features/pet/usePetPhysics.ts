import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isDesktop } from "../../shared/bridge";
import type { PetPresentation, PetPulse } from "../../shared/contracts";
import {
  EMPTY_PET_PHYSICS, PetPhysicsReadinessOwner, parsePetPhysicsState, samePetPhysicsTicket, validPetPhysicsContact,
  type PetPhysicsPaint, type PetPhysicsPaintTicket, type PetPhysicsRenderBinding, type PetPhysicsRendererKind, type PetPhysicsState,
} from "../../shared/petPhysics";
import { allowPetMotionHeartbeat } from "../companion/useMotionReadiness";
import { presentPet } from "./presenter";

type Environment = { visible: boolean; paused: boolean; reduced: boolean };
export interface UsePetPhysicsInput {
  enabled: boolean; presentation: PetPresentation | null; ready: boolean;
  rendererKind: PetPhysicsRendererKind; resourceKey: string;
}
export function usePetPhysics(input: UsePetPhysicsInput) {
  const header = input.presentation as Partial<PetPulse> | null;
  const enabled = input.enabled && isDesktop && new URLSearchParams(window.location.search).get("window") === "pet";
  const scope = JSON.stringify([enabled, input.resourceKey, input.rendererKind, header?.runtimeEpoch, input.presentation?.sourceScopeRevision]);
  const rendererEpoch = useMemo(() => crypto.randomUUID(), [scope]);
  const [state, setState] = useState<PetPhysicsState>(EMPTY_PET_PHYSICS);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [painted, setPainted] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [environment, setEnvironment] = useState<Environment>(() => ({ visible: false, paused: true,
    reduced: window.matchMedia("(prefers-reduced-motion: reduce)").matches }));
  const current = useRef(input); current.current = input;
  const currentScope = useRef(scope); currentScope.current = scope;
  const stateRef = useRef(state);
  const environmentRef = useRef(environment);
  const session = useRef<{ scope: string; owner: PetPhysicsReadinessOwner; paint: PetPhysicsPaint | null; ticket: PetPhysicsPaintTicket | null; held: boolean } | null>(null);
  const busy = useRef(false);
  const waits = useRef(new Set<{ timer: number; finish: () => void }>());
  const refresh = useRef<() => void>(() => {});
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const allowed = () => {
    const value = current.current, host = environmentRef.current;
    return enabled && !document.hidden && host.visible && !host.paused && !host.reduced
      && allowPetMotionHeartbeat(value.presentation, { ready: value.ready, paused: false,
        sourceScopeRevision: value.presentation?.sourceScopeRevision ?? null, hasPrompt: false, tap: false,
        action: presentPet(value.presentation, value.ready).action, interactionBlocked: false }, Date.now());
  };
  const receive = useCallback((unknownState: unknown, expectedScope: string) => {
    if (currentScope.current !== expectedScope) return;
    const value = parsePetPhysicsState(unknownState);
    if (!value) { session.current?.owner.revoke(); setError("桌宠移动状态暂不可用"); return; }
    const previous = stateRef.current;
    if (BigInt(value.revision) < BigInt(previous.revision)) return;
    stateRef.current = value; setState(value); setError(null);
  }, []);

  useEffect(() => {
    if (!enabled) return;
    let live = true, visibilityRevision = 0, runtimeRevision = 0;
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = (patch: Partial<Environment>) => {
      if (!live) return;
      environmentRef.current = { ...environmentRef.current, ...patch };
      setEnvironment(environmentRef.current); refresh.current();
    };
    update({ visible: false, paused: true, reduced: media.matches });
    const visibility = (event: Event) => {
      const detail = (event as CustomEvent).detail;
      if (detail?.label && detail.label !== "pet") return;
      const visible = typeof detail === "boolean" ? detail : detail?.visible;
      if (typeof visible === "boolean") { visibilityRevision++; update({ visible }); }
    };
    const runtime = (event: Event) => {
      const paused = (event as CustomEvent).detail?.paused;
      if (typeof paused === "boolean") { runtimeRevision++; update({ paused }); }
    };
    const initial = (event: Event) => { visibility(event); runtime(event); };
    const restrict = () => refresh.current();
    const reduce = () => update({ reduced: media.matches });
    const hide = () => { session.current?.owner.revoke(); };
    document.addEventListener("visibilitychange", restrict); window.addEventListener("pagehide", hide);
    media.addEventListener("change", reduce);
    window.addEventListener("view-visibility", visibility); window.addEventListener("pet-visibility", visibility);
    window.addEventListener("desktop-runtime-state", runtime); window.addEventListener("initial-view-state", initial);
    const visibilityAt = visibilityRevision, runtimeAt = runtimeRevision;
    void invoke<{ windows?: { pet?: { visible?: boolean; minimized?: boolean } }; runtime?: { paused?: boolean } }>("desktop_metrics").then(metrics => {
      const patch: Partial<Environment> = {};
      if (visibilityAt === visibilityRevision) patch.visible = metrics.windows?.pet?.visible === true && metrics.windows.pet.minimized !== true;
      if (runtimeAt === runtimeRevision) patch.paused = metrics.runtime?.paused !== false;
      update(patch);
    }).catch(() => {});
    return () => {
      live = false; document.removeEventListener("visibilitychange", restrict); window.removeEventListener("pagehide", hide);
      media.removeEventListener("change", reduce); window.removeEventListener("view-visibility", visibility);
      window.removeEventListener("pet-visibility", visibility); window.removeEventListener("desktop-runtime-state", runtime);
      window.removeEventListener("initial-view-state", initial);
    };
  }, [enabled]);

  useEffect(() => {
    setPainted(false); stateRef.current = EMPTY_PET_PHYSICS; setState(EMPTY_PET_PHYSICS); setError(null);
    if (!enabled) return;
    let live = true, remove: (() => void) | undefined;
    const owner = new PetPhysicsReadinessOwner(rendererEpoch, {
      register: () => invoke("register_pet_physics_renderer", { registration: { rendererEpoch, resourceKey: input.resourceKey, rendererKind: input.rendererKind } }),
      send: readiness => invoke("set_pet_physics_readiness", { readiness }),
    });
    const value = { scope, owner, paint: null as PetPhysicsPaint | null, ticket: null as PetPhysicsPaintTicket | null, held: false };
    session.current = value;
    const tick = (heartbeat = false) => {
      if (!live || currentScope.current !== scope) return;
      if (!allowed()) {
        if (value.paint) { value.paint = null; setPainted(false); }
        owner.revoke(); return;
      }
      if (!value.paint) { owner.revoke(); return; }
      const paint = value.paint;
      const ack = value.ticket && samePetPhysicsTicket(paint.ticket, value.ticket)
        && (value.ticket.kind !== "climb" || paint.climbReady) ? { generation: value.ticket.generation, kind: value.ticket.kind } : undefined;
      owner.update({ ready: true, climbReady: input.rendererKind === "vrm" && paint.climbReady, contact: paint.contact, ...(ack ? { ack } : {}) }, undefined, heartbeat);
    };
    refresh.current = tick;
    const timer = window.setInterval(() => tick(true), 500);
    void listen("pet-physics-state", event => { if (live) receive(event.payload, scope); }).then(unsubscribe => {
      if (!live) unsubscribe(); else { remove = unsubscribe; void invoke("get_pet_physics_state").then(value => { if (live) receive(value, scope); }).catch(() => {}); }
    }).catch(() => { if (live) setError("桌宠移动状态暂不可用"); });
    return () => {
      live = false; window.clearInterval(timer); remove?.(); owner.dispose();
      for (const wait of waits.current) { window.clearTimeout(wait.timer); wait.finish(); }
      waits.current.clear();
      if (session.current === value) session.current = null;
      if (refresh.current === tick) refresh.current = () => {};
    };
  }, [scope, rendererEpoch, enabled, receive]);
  useLayoutEffect(() => { refresh.current(); }, [input.presentation, input.ready, environment]);

  const ticket = useMemo<PetPhysicsPaintTicket | null>(() => {
    const request = state.request;
    return enabled && state.rendererEpoch === rendererEpoch && request
      ? { rendererEpoch, resourceKey: input.resourceKey, generation: request.generation, kind: request.kind,
        ...(request.side ? { side: request.side } : {}), ...(request.direction ? { direction: request.direction } : {}) } : null;
  }, [enabled, state.rendererEpoch, state.request?.generation, state.request?.kind, state.request?.side, state.request?.direction, rendererEpoch, input.resourceKey]);
  if (session.current?.scope === scope) session.current.ticket = ticket;
  const holdStill = preparing || !!ticket || state.throwEnabled;
  const binding = useMemo<PetPhysicsRenderBinding>(() => ({ rendererEpoch, resourceKey: input.resourceKey, ticket, holdStill,
    onPainted: paint => {
      const value = session.current;
      if (!value || value.scope !== scope || currentScope.current !== scope || !validPetPhysicsContact(paint.contact)
          || !samePetPhysicsTicket(paint.ticket, value.ticket)) return;
      const changed = !value.paint || !samePetPhysicsTicket(value.paint.ticket, paint.ticket) || value.paint.climbReady !== paint.climbReady;
      value.paint = paint; value.held = holdStill;
      if (changed) { setPainted(true); refresh.current(); }
    },
    onUnavailable: () => {
      const value = session.current;
      if (!value || value.scope !== scope || currentScope.current !== scope) return;
      value.paint = null; value.owner.revoke(); setPainted(false);
    },
  }), [scope, rendererEpoch, input.resourceKey, ticket, holdStill]);
  const perform = useCallback(async (command: string, payload: Record<string, unknown>, requireReady = true) => {
    const value = session.current;
    if (busy.current || !value || value.scope !== scope || value.scope !== currentScope.current || (requireReady && !allowed())) return false;
    const expected = value.scope;
    busy.current = true; setPending(true); if (requireReady) setPreparing(true);
    try {
      if (requireReady) {
        const deadline = performance.now() + 1000;
        // 菜单关闭后的尺寸变化必须等当前角色真实重绘；时间到只报告失败，不延迟执行旧动作。
        while (mounted.current && session.current === value && currentScope.current === expected && allowed()
            && (value.owner.waitingForRevocation() || !value.held || !value.paint || value.paint.contact.viewportWidth !== window.innerWidth || value.paint.contact.viewportHeight !== window.innerHeight)) {
          if (performance.now() >= deadline) throw Error("角色正在恢复显示，请稍后再试");
          await new Promise<void>(resolve => {
            const wait = { timer: 0, finish: () => { waits.current.delete(wait); resolve(); } };
            wait.timer = window.setTimeout(wait.finish, 20); waits.current.add(wait);
          });
        }
        if (!mounted.current || session.current !== value || currentScope.current !== expected || !allowed() || !value.paint) return false;
        const paint = value.paint;
        const accepted = await value.owner.confirm({ ready: true, climbReady: input.rendererKind === "vrm" && paint.climbReady, contact: paint.contact });
        if (!accepted || session.current !== value || currentScope.current !== expected || !allowed() || !value.paint
            || JSON.stringify(value.paint.contact) !== JSON.stringify(paint.contact)
            || value.paint.contact.viewportWidth !== window.innerWidth || value.paint.contact.viewportHeight !== window.innerHeight) {
          if (currentScope.current === expected && allowed()) throw Error("角色画面尚未确认，请稍后再试");
          return false;
        }
      }
      const credentials = value.owner.credentials();
      if (!credentials || session.current !== value || currentScope.current !== expected) return false;
      const nested = payload.request && typeof payload.request === "object" ? { ...payload, request: { ...payload.request, ...credentials } } : payload;
      const result = await invoke(command, { ...nested, ...credentials }); receive(result, expected); return currentScope.current === expected;
    }
    catch (cause) { if (currentScope.current === expected) setError(typeof cause === "string" ? cause : cause instanceof Error ? cause.message : "桌宠移动暂未开始"); return false; }
    finally { busy.current = false; if (mounted.current) { setPending(false); setPreparing(false); } }
  }, [scope, enabled, input.rendererKind, receive]);
  const controls = useMemo(() => ({
    dock: () => {
      return perform("request_pet_physics", { request: { kind: "dock" } });
    },
    climb: (side: "left" | "right", direction: "up" | "down") => {
      return input.rendererKind === "vrm" ? perform("request_pet_physics", { request: { kind: "climb", side, direction } }) : Promise.resolve(false);
    },
    setThrowEnabled: (enabled: boolean) => perform("set_pet_throw_enabled", { enabled }, enabled),
    cancel: () => perform("cancel_pet_physics", {}, false),
  }), [perform, input.rendererKind]);
  const available = enabled && painted && allowed();
  const blockedReason = !enabled ? "桌面互动尚未启用" : environment.reduced || input.presentation?.settings.reducedMotion
    ? "动画已关闭，暂时不可移动" : !available ? "等桌宠空闲并显示后再试" : null;
  return { state, controls, rendererProps: { physics: enabled ? binding : undefined }, available,
    climbAvailable: available && input.rendererKind === "vrm", pending, error, blockedReason };
}
export type PetPhysicsController = ReturnType<typeof usePetPhysics>;
