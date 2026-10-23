import type { DesktopCompanionState } from "../../shared/desktopCompanion";
import { validCompanionState } from "../../shared/desktopCompanion";
import type { AnimationClipDefinition } from "./builtinAnimationSkin";

export type RoamingDirection = "left" | "right";
export interface RoamingAnimation {
  key: string;
  clip: "walk-left" | "walk-right";
  generation: string;
  direction: RoamingDirection;
}

/** 复用已核验侧走的步态帧和播放器；原地预览仍保留完整入场、退出动作。 */
export function roamingPlayback(clips: readonly AnimationClipDefinition[], direction: RoamingDirection): AnimationClipDefinition | null {
  const clip = clips.find((item) => item.id === `walk-${direction}`);
  if (!clip) return null;
  const prefix = direction === "left" ? "walk-v10-complete/frames-left-v12/" : "walk-v10-complete/frames-v11/";
  const body = clip.sequence.filter((step) => step.segment === "body");
  if (body.length !== 16 || body.some((step) => !step.frame.startsWith(prefix)) ||
      new Set(body.slice(0, 8).map((step) => step.frame)).size !== 8 ||
      body.slice(0, 8).some((step, index) => step.frame !== body[index + 8].frame || step.durationMs !== body[index + 8].durationMs)) return null;
  return { ...clip, fallback: body[0].frame, loop: true, loopRange: undefined, sequence: body.slice(0, 8) };
}

/** 仅投影当前原生段，不创建、延长或恢复移动许可。 */
export function roamingAnimation(state: DesktopCompanionState | null | undefined,
  options: { available: boolean; eligible: boolean; scope: string | null; enabled: boolean }): RoamingAnimation | null {
  if (!options.available || !options.eligible || !options.enabled || !options.scope ||
      !validCompanionState(state) || state.phase !== "moving" || state.pauseReason !== null ||
      !state.capabilities.systemIdle || state.inputIdleMs === null || state.inputIdleMs < 120_000) return null;
  const direction = state.motion!.direction;
  return { key: JSON.stringify([options.scope, state.runtimeEpoch, state.motionGeneration, direction]),
    clip: `walk-${direction}`, generation: state.motionGeneration, direction };
}

export function roamingFrameReady(request: RoamingAnimation | null,
  sprite: { current: boolean; clip: string; scope: string | null; runtimeKey: string; error: boolean; frame: string } | null,
  paintedKey: string | null, playback: AnimationClipDefinition | null) {
  return !!request && !!sprite?.current && !sprite.error && sprite.runtimeKey === "builtin" &&
    sprite.clip === request.clip && sprite.scope === request.key && paintedKey === request.key &&
    playback?.sequence.some((step) => step.frame === sprite.frame) === true;
}
