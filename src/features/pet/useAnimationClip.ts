import { useEffect, useRef, useState } from "react";
import { builtinAnimationClips } from "./builtinAnimationSkin";
import { getBuiltinAnimationRegistry } from "./builtinAnimationRegistry";
import { KeyframePlayer, playbackClip, type KeyframeSample } from "./keyframePlayer";
import type { LocalPetSkinRuntime } from "./localPetSkinRuntime";
import { createFrameDisplayGate } from "./frameDisplayGate";

export function useAnimationClip(clipId: string, animate: boolean, enabled = true,
  scope: string | null = null, finite = false, runtime: LocalPetSkinRuntime | null = null,
  beforeDisplay?: (frameKey: string) => Promise<void>) {
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
    const clip = (runtime?.clips ?? builtinAnimationClips).find((item) => item.id === clipId)!;
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
      registry = runtime ? runtime.getRegistry() : getBuiltinAnimationRegistry();
      if (lastRegistry.current !== registry) lastRegistry.current?.cancel();
      lastRegistry.current = registry;
      return registry.load(clipId, !animate);
    };
    void begin()
      .then(async (asset) => {
        if (!live || !asset) return;
        const startedAt = animate ? Date.now() : null;
        const display = async (sample: KeyframeSample, resample?: () => KeyframeSample): Promise<KeyframeSample | null> => {
          while (live) {
            const { frame } = sample, complete = animate && sample.nextInMs === null;
            const image = asset.frames[frame];
            if (!image) throw Error("动画帧未就绪");
            if (await gate(`${runtimeKey}:${frame}`, () => setState((previous) => previous?.clip === clipId &&
              previous.animate === animate && previous.scope === scope && previous.finite === finite && previous.frame === frame &&
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
        if (!animate) { await display({ frame: clip.fallback, nextInMs: null }); return; }
        const advance = async () => {
          if (!live) return;
          try {
            const sample = await display(player.read(performance.now()), () => player.read(performance.now()));
            // Time is sampled afresh after ACK: no delayed frame queue is replayed.
            if (live && sample && sample.nextInMs !== null) timer = window.setTimeout(() => { void advance(); }, sample.nextInMs);
          } catch { failure(); }
        };
        // A failed-result prompt uses the same alert artwork as a persistent
        // uncertain fact, but plays exit/return once instead of repeating body.
        const initial = await display(player.start(playbackClip(clip, finite), performance.now()), () => player.read(performance.now()));
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
  }, [clipId, animate, enabled, scope, finite, runtimeKey, runtime, beforeDisplay]);
  return state ? { ...state, current: enabled && state.clip === clipId && state.animate === animate &&
    state.scope === scope && state.finite === finite && state.runtimeKey === runtimeKey && state.source === runtime } : null;
}
