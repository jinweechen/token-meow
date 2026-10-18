import { useEffect, useRef, useState, type CSSProperties } from "react";
import type { PetPresentation, PetState } from "../../shared/contracts";
import { builtinAnimationManifest as builtinManifest, builtinAnimationClips } from "./builtinAnimationSkin";
import { HD_LOGICAL_POSES, hdPoseFrame } from "./hdPoseLayout";
import { useAnimationClip } from "./useAnimationClip";
import { AnimationController } from "./animationController";
import { presentPet, promptText } from "./presenter";
import { PromptWatermark } from "./promptWatermark";
import { allowsCompanionPose, PoseController, type CutePose } from "./poseController";
import type { DesktopCompanionState } from "../../shared/desktopCompanion";
import { isDesktop } from "../../shared/bridge";
import { allowPetMotionHeartbeat, createPetMotionReadiness, isPetMotionRenderer } from "../companion/useMotionReadiness";
import type { LocalPetSkinRuntime } from "./localPetSkinRuntime";
import { usePetPointerMask } from "./usePetPointerMask";

const clipDuration = (clips: readonly { id: string; sequence: readonly { durationMs: number }[] }[], id: string) => clips.find((clip) => clip.id === id)!
  .sequence.reduce((sum, step) => sum + step.durationMs, 0);

export function CatGirl({
  presentation,
  ready,
  decorative = false,
  forceState,
  onStateText,
  onPromptDelivered,
  companion,
  skinRuntime = null,
  pointerEnabled = false,
}: {
  presentation: PetPresentation | null;
  ready: boolean;
  decorative?: boolean;
  forceState?: PetState;
  onStateText?: (text: string) => void;
  onPromptDelivered?: (id: string) => void;
  companion?: DesktopCompanionState | null;
  skinRuntime?: LocalPetSkinRuntime | null;
  pointerEnabled?: boolean;
}) {
  const [documentVisible, setDocumentVisible] = useState(!document.hidden);
  const [nativeVisible, setNativeVisible] = useState(true);
  const visible = documentVisible && nativeVisible;
  const [runtimePaused, setRuntimePaused] = useState(false);
  const [systemReduced, setSystemReduced] = useState(
    window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );
  const [state, setState] = useState<{
    action: PetState;
    phase: string;
    pose: CutePose | null;
    poseInstance: number | null;
    promptId: string | null;
  }>({
    action: "idle",
    phase: "loop",
    pose: null,
    poseInstance: null,
    promptId: null,
  });
  const clips = skinRuntime?.clips ?? builtinAnimationClips;
  const clipsRef = useRef(clips);
  clipsRef.current = clips;
  const duration = (id: string) => clipDuration(clipsRef.current, id);
  const createAnimationController = () => new AnimationController(duration, true);
  const createPoseController = () => new PoseController(builtinManifest.variants,
    (frame) => duration(HD_LOGICAL_POSES[frame]), true);
  const controller = useRef(createAnimationController());
  const poses = useRef(createPoseController());
  const companionManaged = useRef(companion !== undefined);
  companionManaged.current = companion !== undefined;
  const companionRef = useRef(companion);
  companionRef.current = companion;
  const motionReadiness = useRef<ReturnType<typeof createPetMotionReadiness> | null>(null);
  const interactionFenceUntil = useRef(0);
  const rendererEnabled = isPetMotionRenderer(isDesktop, decorative, window.location.search);
  const presentationRuntimeEpoch = (presentation as { runtimeEpoch?: string } | null)?.runtimeEpoch ?? null;
  const { characterRef, beforeDisplay } = usePetPointerMask(
    rendererEnabled && pointerEnabled && presentation?.settings.alphaHitTest === true,
    JSON.stringify([skinRuntime?.key ?? "builtin", presentation?.sourceScopeRevision, presentationRuntimeEpoch]));
  const readinessDomain = useRef<string | null>(null);
  const updateClock = useRef<(() => void) | null>(null);
  const force = useRef(forceState);
  force.current = forceState;
  const watermark = useRef(new PromptWatermark());
  const current = useRef(presentation);
  const readyRef = useRef(ready);
  readyRef.current = ready;
  const delivered = useRef<string | null>(null);
  const resumeFence = useRef(false),
    lastPrompts = useRef(presentation?.prompts);
  const sourceScope = useRef<string | null>(null);
  const animationDomain = useRef<string | null>(null);
  current.current = presentation;
  const reduced = systemReduced || !!presentation?.settings.reducedMotion;
  const quiet = !!(
    presentation?.settings.effectiveDoNotDisturb ?? presentation?.settings.doNotDisturb
  );
  const motionPaused =
    !visible ||
    runtimePaused ||
    presentation?.settings.petVisible === false ||
    reduced || !ready;
  const paused = motionPaused || quiet;
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  useEffect(() => {
    if (!rendererEnabled) return;
    // StrictMode setup creates a new registration owner after each cleanup.
    const owner = createPetMotionReadiness();
    motionReadiness.current = owner;
    return () => {
      owner.dispose();
      if (motionReadiness.current === owner) motionReadiness.current = null;
    };
  }, [rendererEnabled]);
  useEffect(() => {
    const domain = presentationRuntimeEpoch && presentation?.sourceScopeRevision
      ? JSON.stringify([presentationRuntimeEpoch, presentation.sourceScopeRevision]) : null;
    if (readinessDomain.current !== domain) {
      if (readinessDomain.current !== null) interactionFenceUntil.current = Date.now() + 500;
      motionReadiness.current?.update({ ready: false, gaitAvailable: false, gaitReady: false });
      readinessDomain.current = domain;
    }
    if (paused || !ready || !presentation || (companionManaged.current && !companion))
      motionReadiness.current?.update({ ready: false, gaitAvailable: false, gaitReady: false });
  }, [presentationRuntimeEpoch, presentation?.sourceScopeRevision, paused, ready, companion]);
  useEffect(() => {
    const change = () => setDocumentVisible(!document.hidden);
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const mediaChange = () => setSystemReduced(mq.matches);
    const visibility = (event: Event) => {
      const detail = (event as CustomEvent).detail;
      if (typeof detail === "boolean" ? !detail : detail?.visible === false)
        motionReadiness.current?.update({ ready: false, gaitAvailable: false, gaitReady: false });
      setNativeVisible(
        typeof detail === "boolean" ? detail : detail?.visible !== false,
      );
    };
    const runtime = (event: Event) => {
      const paused = (event as CustomEvent).detail?.paused === true;
      if (paused) motionReadiness.current?.update({ ready: false, gaitAvailable: false, gaitReady: false });
      setRuntimePaused(paused);
    };
    const interaction = () => {
      interactionFenceUntil.current = Date.now() + 500;
      motionReadiness.current?.update({ ready: false, gaitAvailable: false, gaitReady: false });
      controller.current.interrupt();
      poses.current.interrupt();
      updateClock.current?.();
    };
    const affection = () => {
      motionReadiness.current?.update({ ready: false, gaitAvailable: false, gaitReady: false });
      const now = Date.now(),
        intent = presentPet(current.current, readyRef.current);
      const animation = controller.current.tick(
        force.current ?? intent.action,
        now,
      );
      poses.current.tap(
        !pausedRef.current &&
          animation.action === "idle" &&
          !animation.prompt &&
          intent.active === 0 &&
          intent.uncertain === 0,
        now,
      );
      updateClock.current?.();
    };
    document.addEventListener("visibilitychange", change);
    mq.addEventListener("change", mediaChange);
    window.addEventListener("pet-visibility", visibility);
    window.addEventListener("view-visibility", visibility);
    window.addEventListener("desktop-runtime-state", runtime);
    window.addEventListener("pet-interaction", interaction);
    window.addEventListener("pet-affection", affection);
    return () => {
      document.removeEventListener("visibilitychange", change);
      mq.removeEventListener("change", mediaChange);
      window.removeEventListener("pet-visibility", visibility);
      window.removeEventListener("view-visibility", visibility);
      window.removeEventListener("desktop-runtime-state", runtime);
      window.removeEventListener("pet-interaction", interaction);
      window.removeEventListener("pet-affection", affection);
    };
  }, []);
  useEffect(() => {
    controller.current.suppress(paused);
    if (paused) resumeFence.current = true;
    // Null/loading is not a confirmed empty arrival batch.
    if (!presentation || !ready) return;
    const domain = JSON.stringify([presentationRuntimeEpoch, presentation.sourceScopeRevision]);
    if (animationDomain.current !== domain) {
      animationDomain.current = domain;
      sourceScope.current = presentation.sourceScopeRevision;
      watermark.current.reset();
      controller.current.interrupt();
      poses.current.interrupt();
      delivered.current = null;
      updateClock.current?.();
    }
    const suppressBatch = paused || decorative || resumeFence.current;
    for (const event of watermark.current.consume(
      presentation.prompts,
      Date.now(),
      suppressBatch,
    ))
      controller.current.enqueue(event, Date.now());
    if (!paused && resumeFence.current && presentation.prompts !== lastPrompts.current)
      resumeFence.current = false;
    lastPrompts.current = presentation.prompts;
  }, [presentation?.prompts, presentation?.sourceScopeRevision, presentationRuntimeEpoch, paused, ready]);
  useEffect(() => {
    controller.current.suppress(paused);
    const update = () => {
      const now = Date.now();
      const intent = presentPet(current.current, readyRef.current);
      const next = controller.current.tick(forceState ?? intent.action, now);
      const eligible = !paused &&
          next.action === "idle" &&
          !next.prompt &&
          intent.active === 0 &&
          intent.uncertain === 0;
      const desktop = companionRef.current;
      const companionIdle = allowsCompanionPose(desktop, current.current?.settings.idlePlayEnabled !== false);
      if (!companionIdle) poses.current.stopCompanion();
      if (desktop?.phase === "playing" && builtinManifest.variants.idle.length) {
        const variants = builtinManifest.variants.idle;
        poses.current.play(eligible && companionIdle,
          JSON.stringify([desktop.runtimeEpoch, desktop.motionGeneration]),
          variants[Number(BigInt(desktop.motionGeneration) % BigInt(variants.length))], now);
      }
      const pose = poses.current.tick(eligible, now,
        !companionManaged.current && current.current?.settings.idlePlayEnabled !== false);
      const poseInstance = poses.current.playbackKey();
      const promptId = next.prompt?.id ?? null;
      motionReadiness.current?.update({
        ready: allowPetMotionHeartbeat(current.current, {
          ready: readyRef.current, paused: pausedRef.current, sourceScopeRevision: sourceScope.current,
          hasPrompt: !!next.prompt, tap: pose?.kind === "tap", action: next.action,
          interactionBlocked: now < interactionFenceUntil.current ||
            (companionManaged.current && !companionRef.current),
        }, now),
        // The walk clip is an in-place demonstration; native gait remains unavailable.
        gaitAvailable: false,
        gaitReady: false,
      });
      if (next.prompt && delivered.current !== next.prompt.id) {
        delivered.current = next.prompt.id;
        onPromptDelivered?.(next.prompt.id);
      }
      setState((previous) =>
        previous.action === next.action &&
        previous.phase === next.phase &&
        previous.promptId === promptId &&
        previous.poseInstance === poseInstance &&
        previous.pose?.frame === pose?.frame &&
        previous.pose?.kind === pose?.kind
          ? previous
          : { action: next.action, phase: next.phase, pose, poseInstance, promptId },
      );
      onStateText?.(
        next.prompt
          ? `${promptText(next.prompt)}${intent.active > 0 ? ` · ${intent.active} 个回合仍活动中` : ""}`
          : intent.text,
      );
    };
    updateClock.current = update;
    update();
    if (paused)
      return () => {
        if (updateClock.current === update) updateClock.current = null;
      };
    const timer = window.setInterval(update, 250);
    return () => {
      window.clearInterval(timer);
      if (updateClock.current === update) updateClock.current = null;
    };
  }, [
    paused,
    ready,
    quiet,
    forceState,
    presentation?.activity.activeTurns,
    presentation?.activity.uncertainTurns,
    presentation?.sourceScopeRevision,
    presentation?.settings.idlePlayEnabled,
    companion?.runtimeEpoch,
    companion?.revision,
    onStateText,
    onPromptDelivered,
  ]);
  useEffect(() => {
    const fresh = createAnimationController();
    const freshPoses = createPoseController();
    controller.current = fresh;
    poses.current = freshPoses;
    return () => {
      fresh.destroy();
      freshPoses.destroy();
    };
  }, []);
  const intent = presentPet(presentation, ready);
  const currentScope = sourceScope.current === presentation?.sourceScopeRevision &&
    animationDomain.current === JSON.stringify([presentationRuntimeEpoch, presentation?.sourceScopeRevision]);
  const semanticFrame = hdPoseFrame(
    builtinManifest,
    forceState ?? (currentScope && state.promptId ? state.action : intent.action),
    currentScope ? state.phase : "loop",
    motionPaused,
    quiet,
  );
  const displayedPose = currentScope && intent.active === 0 && intent.uncertain === 0 &&
    (state.pose?.kind !== "idle" || !companionManaged.current ||
      allowsCompanionPose(companion, presentation?.settings.idlePlayEnabled !== false)) ? state.pose : null;
  const frame =
    semanticFrame && displayedPose && !paused && semanticFrame.action === "idle"
      ? { ...semanticFrame, frame: displayedPose.frame }
      : semanticFrame;
  const requestedPose = HD_LOGICAL_POSES[frame.frame];
  const sprite = useAnimationClip(requestedPose, !paused,
    visible && !runtimePaused && presentation?.settings.petVisible !== false,
    JSON.stringify([presentation?.sourceScopeRevision ?? null, presentationRuntimeEpoch,
      currentScope ? state.promptId : null, displayedPose ? state.poseInstance : null]),
    currentScope && state.promptId !== null, skinRuntime, beforeDisplay);
  useEffect(() => {
    if (!sprite?.current || paused || !currentScope) return;
    if (sprite.error || sprite.complete) {
      if (state.poseInstance !== null) poses.current.finishPlayback(state.poseInstance);
      if (state.promptId) controller.current.finishPlayback(state.promptId);
      if (state.poseInstance !== null || state.promptId) updateClock.current?.();
    } else if (sprite.startedAt !== null) {
      if (state.poseInstance !== null) poses.current.startPlayback(state.poseInstance, sprite.startedAt);
      if (state.promptId) controller.current.startPlayback(state.promptId, sprite.startedAt);
    }
  }, [sprite, paused, currentScope, state.poseInstance, state.promptId]);
  const error = sprite?.error === true;
  const loaded = !!sprite?.url && !error;
  return (
    <div
      ref={characterRef}
      className={`character uses-animation-clips state-${frame?.action ?? intent.action} phase-${state.phase} ${displayedPose && !paused ? `pose-${displayedPose.kind}` : ""} ${paused || error ? "motion-paused" : ""}`}
      aria-hidden={decorative || undefined}
      role={decorative ? undefined : "img"}
      aria-label={decorative ? undefined : `${skinRuntime?.manifest.name ?? "小柚"}，${intent.text}`}
    >
      {loaded && sprite ? (
        <div
          className="pet-sprite pet-hd-sprite"
          data-frame={frame.frame}
          data-hd-pose={requestedPose}
          data-animation-clip={sprite.clip}
          data-animation-frame={sprite.frame}
          data-pose={displayedPose && !paused ? displayedPose.kind : undefined}
          style={
            {
              aspectRatio: "1024 / 1536",
              "--pet-frame-ratio": 1024 / 1536,
            } as CSSProperties
          }
        >
          <img src={sprite.url} alt="" draggable={false} data-pointer-frame-key={`${sprite.runtimeKey}:${sprite.frame}`}
            onError={() => { if (sprite.current) skinRuntime?.reportFailure(); }} style={{
            position: "absolute", maxHeight: "none",
            width: `${sprite.sharedTransform.scale * 100}%`,
            left: `${sprite.sharedTransform.translate[0] / 1024 * 100}%`,
            top: `${sprite.sharedTransform.translate[1] / 1536 * 100}%`,
          }} />
        </div>
      ) : (
        <svg viewBox="0 0 280 420" aria-label="角色加载占位">
          <g transform="translate(80 140)" fill="none" stroke="#86ad9f" strokeWidth="4" strokeLinecap="round" strokeLinejoin="round">
            <path d="M8 48 12 6 44 26Q60 20 76 26L108 6 112 48Q132 112 60 118-12 112 8 48Z" />
            <path d="M30 66h5m50 0h5M51 89q9 9 18 0" />
          </g>
        </svg>
      )}
      {!loaded && (
        <span className="asset-fallback">{error ? "角色暂不可用" : "正在加载角色"} · {intent.text}</span>
      )}
    </div>
  );
}
