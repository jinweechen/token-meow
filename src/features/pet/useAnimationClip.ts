import { useEffect, useRef, useState } from "react";
import { builtinAnimationClips } from "./builtinAnimationSkin";
import { getBuiltinAnimationRegistry } from "./builtinAnimationRegistry";
import { KeyframePlayer, playbackClip, type KeyframePosition } from "./keyframePlayer";
import { AnimationClipRegistry, loadAnimationWindow, planAnimationWindow } from "./animationClipRegistry";
import type { LocalPetSkinRuntime } from "./localPetSkinRuntime";
import { createFrameDisplayGate } from "./frameDisplayGate";

export function useAnimationClip(clipId: string, animate: boolean, enabled = true,
  scope: string | null = null, finite = false, runtime: LocalPetSkinRuntime | null = null,
  beforeDisplay?: (frameKey: string) => Promise<void>, registryOverride?: AnimationClipRegistry) {
  const runtimeKey = runtime?.key ?? "builtin";
  const effectOwner = useRef(0);
  const lastRegistry = useRef<ReturnType<typeof getBuiltinAnimationRegistry> | null>(null);
  const [state, setState] = useState<{
    clip: string; animate: boolean; scope: string | null; frame: string; url: string; error: boolean;
    startedAt: number | null; complete: boolean;
    finite: boolean;
    runtimeKey: string;
    source: LocalPetSkinRuntime | null;
    sharedTransform: { scale: number; translate: readonly [number, number] };
  } | null>(null);
  useEffect(() => {
    let live = true;
    const owner = ++effectOwner.current;
    let timer: number | undefined;
    let cancelWait: (() => void) | undefined;
    let registry: ReturnType<typeof getBuiltinAnimationRegistry> | undefined;
    const player = new KeyframePlayer();
    // The installed local-PNG v1 contract keeps its original thirteen logical
    // names. New built-in directions must never widen that untrusted contract.
    const localId = clipId === "walk-right" ? "walk" : clipId;
    const actualId = runtime ? runtime.clips.some((clip) => clip.id === localId) ? localId : "idle" : clipId;
    const clip = (runtime?.clips ?? builtinAnimationClips).find((item) => item.id === actualId)!;
    const windowed = new Set([clip.fallback, ...clip.sequence.map((step) => step.frame)]).size > 12;
    runtime?.setBeforeRelease(beforeDisplay ? () => beforeDisplay(`${runtimeKey}:release`) : undefined);
    const gate = createFrameDisplayGate(beforeDisplay, () => live);
    const waitToRetry = () => new Promise<void>((resolve) => {
      cancelWait = resolve;
      timer = window.setTimeout(() => { cancelWait = undefined; resolve(); }, 500);
    });
    const retryGate = async (key: string, commit: () => void) => {
      while (live) {
        if (await gate(key, commit)) return true;
        if (!live) break;
        await waitToRetry();
      }
      return false;
    };
    const failure = () => {
      if (!live) return;
      if (runtime) runtime.reportFailure();
      else void retryGate(`${runtimeKey}:error`, () => setState({ clip: clipId, animate, scope, frame: clip.fallback, url: "", error: true,
        startedAt: null, complete: true, finite, runtimeKey, sharedTransform: clip.sharedTransform, source: runtime }));
    };
    // The old image/URL remain until native hit-region clearing has acknowledged.
    const begin = async () => {
      if (!await retryGate(`${runtimeKey}:loading`, () => setState(null))) return null;
      if (!enabled) {
        lastRegistry.current?.cancel();
        lastRegistry.current = null;
        return null;
      }
      registry = registryOverride ?? (runtime ? runtime.getRegistry() : getBuiltinAnimationRegistry());
      if (lastRegistry.current !== registry) lastRegistry.current?.cancel();
      lastRegistry.current = registry;
      return animate && windowed ? registry.loadWindow(actualId, planAnimationWindow(clip, 0))
        : registry.load(actualId, !animate);
    };
    void begin()
      .then(async (initialAsset) => {
        if (!live || !initialAsset) return;
        let asset = initialAsset;
        const startedAt = animate ? Date.now() : null;
        const display = async (sample: KeyframePosition, resample?: () => KeyframePosition): Promise<KeyframePosition | null> => {
          while (live) {
            if (!asset.frames[sample.frame]) {
              // Hold at the current presentation phase throughout ACK and serial
              // IO. Window loading must not skip the rest of a 23-frame turn.
              if (sample.nextInMs !== null) sample = player.pause(performance.now());
              if (!asset.frames[sample.frame]) {
                const next = await loadAnimationWindow(registry!, actualId,
                  planAnimationWindow(clip, sample.stepIndex, sample.frame),
                  () => retryGate(`${runtimeKey}:window:${sample.stepIndex ?? "end"}`, () => setState(null)),
                  () => live && effectOwner.current === owner);
                if (!live || !next) return null;
                asset = next;
              }
              if (sample.nextInMs !== null) sample = player.resume(performance.now());
            }
            const { frame } = sample, complete = animate && sample.nextInMs === null;
            const image = asset.frames[frame];
            if (!image) throw Error("动画帧未就绪");
            if (await gate(`${runtimeKey}:${frame}`, () => setState((previous) => previous?.clip === clipId &&
              previous.animate === animate && previous.scope === scope && previous.finite === finite && previous.frame === frame &&
              previous.url === image.url &&
              previous.runtimeKey === runtimeKey && previous.source === runtime &&
              previous.complete === complete && !previous.error
                ? previous : { clip: clipId, animate, scope, frame, url: image.url, error: false, startedAt, complete, finite, runtimeKey,
                  sharedTransform: clip.sharedTransform, source: runtime }))) return sample;
            if (!live) break;
            await waitToRetry();
            // Retry the current clock position; an unsuccessful ACK never queues
            // a stale frame, including when a finite clip ended while waiting.
            if (live && resample && sample.nextInMs !== null) sample = resample();
          }
          return null;
        };
        if (!animate) { await display({ frame: clip.fallback, nextInMs: null, stepIndex: null }); return; }
        const advance = async () => {
          if (!live) return;
          try {
            const sample = await display(player.readPosition(performance.now()), () => player.readPosition(performance.now()));
            // Time is sampled afresh after ACK: no delayed frame queue is replayed.
            if (live && sample && sample.nextInMs !== null) timer = window.setTimeout(() => { void advance(); }, sample.nextInMs);
          } catch { failure(); }
        };
        // A failed-result prompt uses the same alert artwork as a persistent
        // uncertain fact, but plays exit/return once instead of repeating body.
        const start = performance.now();
        player.start(playbackClip(clip, finite), start);
        const initial = await display(player.readPosition(start), () => player.readPosition(performance.now()));
        if (live && initial && initial.nextInMs !== null) timer = window.setTimeout(() => { void advance(); }, initial.nextInMs);
      })
      .catch(failure);
    return () => {
      live = false;
      player.stop();
      cancelWait?.(); cancelWait = undefined;
      window.clearTimeout(timer);
      // A replacement effect owns the same registry; its load handles cancellation.
      // On final unmount, clear the native region before releasing this owner's URLs.
      void Promise.resolve().then(() => {
        if (effectOwner.current !== owner) return;
        return beforeDisplay?.(`${runtimeKey}:cleanup`);
      }).catch(() => {}).finally(() => {
        if (effectOwner.current === owner) {
          registry?.cancel();
          if (lastRegistry.current === registry) lastRegistry.current = null;
        }
      });
    };
  }, [clipId, animate, enabled, scope, finite, runtimeKey, runtime, beforeDisplay, registryOverride]);
  return state ? { ...state, current: enabled && state.clip === clipId && state.animate === animate &&
    state.scope === scope && state.finite === finite && state.runtimeKey === runtimeKey && state.source === runtime } : null;
}
