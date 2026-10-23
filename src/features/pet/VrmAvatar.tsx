import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { PetState } from "../../shared/contracts";
import type { PetPhysicsPaint, PetPhysicsPaintTicket } from "../../shared/petPhysics";
import { VrmRenderer, type VrmRenderIntent } from "./vrmRenderer";

export interface VrmAvatarProps {
  model: { modelId: string; name: string; bytes: number };
  readModel: (id: string) => Promise<ArrayBuffer>;
  action?: PetState;
  reaction?: string | null;
  reducedMotion?: boolean;
  paused?: boolean;
  onReady?: (modelId: string) => void;
  onFailure?: (message: string) => void;
  physicsTicket?: PetPhysicsPaintTicket | null;
  onPainted?: (paint: PetPhysicsPaint) => void;
}

export function VrmAvatar({ model, readModel, action = "idle", reaction = null, reducedMotion = false, paused = false, onReady, onFailure, physicsTicket = null, onPainted }: VrmAvatarProps) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const owner = useRef<VrmRenderer | null>(null);
  const generation = useRef(0);
  const identity = useRef({ modelId: model.modelId, bytes: model.bytes, readModel });
  identity.current = { modelId: model.modelId, bytes: model.bytes, readModel };
  const callbacks = useRef({ onReady, onFailure, onPainted });
  callbacks.current = { onReady, onFailure, onPainted };
  const intent = useRef({ action, reaction, reducedMotion, paused, physicsTicket });
  intent.current = { action, reaction, reducedMotion, paused, physicsTicket };
  const synchronize = useRef<(() => void) | null>(null);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");

  useEffect(() => {
    if (!canvas.current) return;
    const currentGeneration = ++generation.current;
    const current = () => generation.current === currentGeneration && identity.current.modelId === model.modelId
      && identity.current.bytes === model.bytes && identity.current.readModel === readModel;
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    setStatus("loading");
    const instance = new VrmRenderer(canvas.current, {
      onReady: () => {
        if (!current()) return;
        setStatus("ready"); callbacks.current.onReady?.(model.modelId);
      },
      onFailure: (message) => {
        if (!current()) return;
        setStatus("error"); callbacks.current.onFailure?.(message);
      },
      onPainted: paint => { if (current()) callbacks.current.onPainted?.(paint); },
    });
    owner.current = instance;
    const sync = () => {
      const environment: VrmRenderIntent = { ...intent.current, reducedMotion: intent.current.reducedMotion || media.matches, visible: !document.hidden };
      instance.setIntent(environment);
    };
    synchronize.current = sync;
    document.addEventListener("visibilitychange", sync);
    media.addEventListener("change", sync);
    sync();
    void (async () => {
      try {
        const bytes = await readModel(model.modelId);
        if (!current()) return;
        sync();
        await instance.load(bytes, model.bytes);
      } catch (error) {
        if (!current()) return;
        instance.dispose();
        setStatus("error"); callbacks.current.onFailure?.(error instanceof Error ? error.message : "无法读取本地 VRM 模型");
      }
    })();
    return () => {
      ++generation.current;
      document.removeEventListener("visibilitychange", sync);
      media.removeEventListener("change", sync);
      if (synchronize.current === sync) synchronize.current = null;
      if (owner.current === instance) owner.current = null;
      instance.dispose();
    };
  }, [model.modelId, model.bytes, readModel]);

  useLayoutEffect(() => { synchronize.current?.(); }, [action, reaction, reducedMotion, paused, physicsTicket]);

  return <canvas ref={canvas} className="vrm-avatar" role="img" aria-label={model.name} data-vrm-state={status}
    style={{ display: "block", width: "100%", height: "100%", background: "transparent", pointerEvents: "none" }} />;
}
