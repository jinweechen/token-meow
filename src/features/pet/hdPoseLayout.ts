type Action = "idle" | "active" | "complete" | "alert" | "sleep";

export const HD_LOGICAL_POSES = Object.freeze([
  "idle", "active", "complete", "alert", "sleep", "idle", "shy",
  "affection", "stretch", "wink", "happy", "bow", "hug-tail", "walk-right",
  "walk-left", "walk-left-to-right", "walk-right-to-left",
] as const);

export function hdPoseFrame(
  manifest: { actions: Record<Action, { frames: { start: number; loop: number; end: number } }> },
  action: Action, phase: string, paused: boolean, quiet: boolean,
) {
  const displayed = quiet && !paused ? "sleep" : action;
  const displayedPhase = quiet && !paused ? "loop" : phase;
  const frame = paused || displayedPhase === "static" ? 5 : manifest.actions[displayed].frames[
    displayedPhase === "start" || displayedPhase === "end" ? displayedPhase : "loop"
  ];
  return { action: displayed, frame };
}

