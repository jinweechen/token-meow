import { Component, Suspense, lazy, useEffect, useLayoutEffect, useRef, useState, type ErrorInfo, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { PetPresentation } from "../../shared/contracts";
import type { VrmSkinModel } from "../../shared/vrmSkin";
import { isDesktop } from "../../shared/bridge";
import { VrmPetPresentation, vrmPetDomain, type VrmPetFrame, type VrmPetInput } from "./vrmPetPresentation";
import { presentPet } from "./presenter";
import { samePetPhysicsTicket, type PetPhysicsRenderBinding } from "../../shared/petPhysics";

const VrmAvatar = lazy(() => import("./VrmAvatar").then(module => ({ default: module.VrmAvatar })));

type Environment = { documentVisible: boolean; nativeVisible: boolean; paused: boolean; reduced: boolean };

function usePetEnvironment(notify: () => void, hostEnvironment?: { visible: boolean; paused: boolean }) {
  const [value, setValue] = useState<Environment>(() => ({
    documentVisible: !document.hidden,
    nativeVisible: hostEnvironment?.visible ?? !isDesktop,
    paused: hostEnvironment?.paused ?? isDesktop,
    reduced: window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  }));
  const current = useRef(value);
  const notifyCurrent = useRef(notify);
  notifyCurrent.current = notify;
  useEffect(() => {
    let live = true, visibilityRevision = 0, runtimeRevision = 0;
    const update = (patch: Partial<Environment>) => {
      current.current = { ...current.current, ...patch };
      if (live) setValue(current.current);
      if (live) notifyCurrent.current();
    };
    const documentVisibility = () => update({ documentVisible: !document.hidden });
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const preference = () => update({ reduced: media.matches });
    const visibility = (event: Event) => {
      const detail = (event as CustomEvent).detail;
      if (detail?.label && detail.label !== "pet") return;
      const visible = typeof detail === "boolean" ? detail : detail?.visible;
      if (typeof visible === "boolean") { visibilityRevision++; update({ nativeVisible: visible }); }
    };
    const runtime = (event: Event) => {
      const paused = (event as CustomEvent).detail?.paused;
      if (typeof paused === "boolean") { runtimeRevision++; update({ paused }); }
    };
    const initial = (event: Event) => { visibility(event); runtime(event); };
    document.addEventListener("visibilitychange", documentVisibility);
    media.addEventListener("change", preference);
    window.addEventListener("view-visibility", visibility);
    window.addEventListener("pet-visibility", visibility);
    window.addEventListener("desktop-runtime-state", runtime);
    window.addEventListener("initial-view-state", initial);
    if (isDesktop && !hostEnvironment) {
      const requestedVisibility = visibilityRevision, requestedRuntime = runtimeRevision;
      void invoke<{ windows?: { pet?: { visible?: boolean; minimized?: boolean } }; runtime?: { paused?: boolean } }>("desktop_metrics").then(metrics => {
        if (!live) return;
        const patch: Partial<Environment> = {};
        if (requestedVisibility === visibilityRevision) patch.nativeVisible = metrics.windows?.pet?.visible === true && metrics.windows.pet.minimized !== true;
        if (requestedRuntime === runtimeRevision) patch.paused = metrics.runtime?.paused !== false;
        update(patch);
      }).catch(() => {});
    }
    return () => {
      live = false;
      document.removeEventListener("visibilitychange", documentVisibility);
      media.removeEventListener("change", preference);
      window.removeEventListener("view-visibility", visibility);
      window.removeEventListener("pet-visibility", visibility);
      window.removeEventListener("desktop-runtime-state", runtime);
      window.removeEventListener("initial-view-state", initial);
    };
  }, []);
  useEffect(() => {
    if (!hostEnvironment) return;
    current.current = { ...current.current, nativeVisible: hostEnvironment.visible, paused: hostEnvironment.paused };
    setValue(current.current); notifyCurrent.current();
  }, [hostEnvironment?.visible, hostEnvironment?.paused]);
  return { value, current };
}

class VrmBoundary extends Component<{ children: ReactNode; onFailure: (message: string) => void }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  componentDidCatch(error: Error, _info: ErrorInfo) { this.props.onFailure(error.message || "VRM 渲染组件未能加载。"); }
  render() { return this.state.failed ? null : this.props.children; }
}

export function VrmPetCharacter({ model, readModel, presentation, ready, onStateText, onPromptDelivered, onFailure, hostEnvironment, physics }: {
  model: VrmSkinModel;
  readModel: (id: string) => Promise<ArrayBuffer>;
  presentation: PetPresentation | null;
  ready: boolean;
  onStateText?: (text: string) => void;
  onPromptDelivered?: (id: string) => void;
  onFailure?: (message: string) => void;
  hostEnvironment?: { visible: boolean; paused: boolean };
  physics?: PetPhysicsRenderBinding;
}) {
  const wake = useRef<() => void>(() => {});
  const environment = usePetEnvironment(() => wake.current(), hostEnvironment);
  const owner = useRef(new VrmPetPresentation());
  const generation = useRef({ modelId: model.modelId, value: 1 });
  const painted = useRef<number | null>(null);
  const failed = useRef<number | null>(null);
  if (generation.current.modelId !== model.modelId) {
    generation.current = { modelId: model.modelId, value: generation.current.value + 1 };
    painted.current = null;
    failed.current = null;
  }
  const epoch = generation.current.value;
  const [paintedEpoch, setPaintedEpoch] = useState<number | null>(null);
  const [failureEpoch, setFailureEpoch] = useState<number | null>(null);
  const latest = useRef({ model, presentation, ready, onStateText, onPromptDelivered, onFailure });
  latest.current = { model, presentation, ready, onStateText, onPromptDelivered, onFailure };
  const input = (): VrmPetInput => {
    const current = latest.current;
    const host = environment.current.current;
    const quiet = current.presentation?.settings.effectiveDoNotDisturb ?? current.presentation?.settings.doNotDisturb;
    return {
      modelId: current.model.modelId, presentation: current.presentation, ready: current.ready,
      blocked: document.hidden || !host.documentVisible || !host.nativeVisible || host.paused || !!quiet || current.presentation?.settings.petVisible === false || failed.current === generation.current.value,
      reducedMotion: host.reduced || current.presentation?.settings.reducedMotion === true,
    };
  };
  const [frame, setFrame] = useState<VrmPetFrame>(() => ({
    domain: null, action: presentPet(presentation, ready).action, reaction: null, promptId: null,
    text: presentPet(presentation, ready).text, paused: true,
  }));
  const update = useRef<() => void>(() => {});
  update.current = () => {
    const next = owner.current.observe(input(), Date.now());
    setFrame(previous => JSON.stringify(previous) === JSON.stringify(next) ? previous : next);
    latest.current.onStateText?.(next.text);
  };
  wake.current = () => update.current();
  useEffect(() => {
    const fresh = new VrmPetPresentation();
    owner.current = fresh;
    return () => fresh.dispose();
  }, []);
  useEffect(() => {
    update.current();
    const affection = (event: Event) => {
      const next = owner.current.tap(input(), Date.now(), (event as CustomEvent<{ pose?: unknown }>).detail?.pose);
      setFrame(next);
      latest.current.onStateText?.(next.text);
    };
    const interrupt = () => { owner.current.interrupt(); update.current(); };
    window.addEventListener("pet-affection", affection);
    window.addEventListener("pet-interaction", interrupt);
    return () => {
      window.removeEventListener("pet-affection", affection);
      window.removeEventListener("pet-interaction", interrupt);
    };
  }, []);
  useEffect(() => {
    update.current();
    const current = input();
    if (current.blocked || current.reducedMotion || !current.ready || !current.presentation) return;
    const timer = window.setInterval(() => update.current(), 250);
    return () => window.clearInterval(timer);
  }, [presentation, ready, model.modelId, environment.value, paintedEpoch, failureEpoch]);
  useEffect(() => {
    if (!frame.promptId || frame.paused || painted.current !== epoch || paintedEpoch !== epoch) return;
    let first = 0, second = 0, live = true;
    // 等待对应 action 提交和实际渲染机会，再确认有限提示；隐藏或换域立即阻止旧确认。
    first = requestAnimationFrame(() => {
      second = requestAnimationFrame(() => {
        if (!live || generation.current.value !== epoch || painted.current !== epoch || failed.current === epoch) return;
        const current = owner.current.observe(input(), Date.now());
        if (current.domain !== frame.domain || current.promptId !== frame.promptId || current.paused) return;
        if (owner.current.acknowledge(frame.domain, frame.promptId!, Date.now())) latest.current.onPromptDelivered?.(frame.promptId!);
        update.current();
      });
    });
    return () => { live = false; cancelAnimationFrame(first); cancelAnimationFrame(second); };
  }, [frame.domain, frame.promptId, frame.paused, paintedEpoch, epoch]);
  const fail = (message: string) => {
    if (generation.current.value !== epoch || failed.current === epoch) return;
    failed.current = epoch; painted.current = null;
    owner.current.interrupt(); setFailureEpoch(epoch);
    physics?.onUnavailable();
    latest.current.onFailure?.(message);
  };
  const rendering = input();
  const intent = presentPet(presentation, ready);
  const domain = vrmPetDomain(rendering);
  const paused = rendering.blocked || rendering.reducedMotion || !ready || !presentation || !domain;
  const displayed = frame.domain === domain && !paused
    ? !frame.promptId && (intent.active > 0 || intent.uncertain > 0)
      ? { ...frame, action: intent.action, reaction: null, text: intent.text } : frame
    : { domain, action: intent.action, reaction: null, promptId: null, text: intent.text, paused: true };
  const reduced = environment.value.reduced || presentation?.settings.reducedMotion === true;
  const physicsEligible = !paused && !displayed.paused && displayed.action === "idle" && !displayed.promptId && !displayed.reaction;
  const physicsFrame = useRef({ physics, eligible: physicsEligible });
  physicsFrame.current = { physics, eligible: physicsEligible };
  useLayoutEffect(() => { if (!physicsEligible) physics?.onUnavailable(); }, [physicsEligible, physics]);
  useEffect(() => () => physics?.onUnavailable(), [physics?.rendererEpoch]);
  return <div className={`character pet-vrm-character state-${displayed.action} ${paused ? "motion-paused" : ""}`}
      role="img" aria-label={`${model.name}，${displayed.text}`} data-vrm-action={displayed.action} data-vrm-reaction={displayed.reaction ?? undefined}>
    <VrmBoundary key={epoch} onFailure={fail}>
      <Suspense fallback={null}>
        <VrmAvatar key={`${model.modelId}:${epoch}`} model={model} readModel={readModel} action={displayed.action}
          reaction={paused ? null : displayed.reaction} paused={paused} reducedMotion={reduced}
          physicsTicket={physicsEligible ? physics?.ticket : null}
          onPainted={paint => {
            const current = physicsFrame.current;
            if (generation.current.value !== epoch || failed.current === epoch || !current.eligible || !current.physics
                || !samePetPhysicsTicket(paint.ticket, current.physics.ticket)) return;
            current.physics.onPainted(paint);
          }}
          onReady={id => {
            if (id !== latest.current.model.modelId || generation.current.value !== epoch || failed.current === epoch) return;
            painted.current = epoch; setPaintedEpoch(epoch); update.current();
          }} onFailure={fail} />
      </Suspense>
    </VrmBoundary>
  </div>;
}
