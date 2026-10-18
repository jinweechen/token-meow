export interface KeyframeClip {
  id: string;
  fallback: string;
  loop: boolean;
  /** Entry is played once; only these body steps repeat while the state stays active. */
  loopRange?: { start: number; end: number };
  sequence: readonly { frame: string; durationMs: number }[];
}

export interface KeyframeSample {
  frame: string;
  nextInMs: number | null;
}

/** Static rendering keeps the catalog fallback; finite playback keeps its final return pose. */
export function playbackClip(clip: KeyframeClip, finite = false): KeyframeClip {
  return finite || !clip.loop ? { ...clip, loop: false, loopRange: undefined,
    fallback: clip.sequence.at(-1)?.frame ?? clip.fallback } : clip;
}

/** A presentation clock. It never starts or extends a business action. */
export class KeyframePlayer {
  private clip: KeyframeClip | null = null;
  private started = 0;
  private previous = 0;

  start(clip: KeyframeClip, now: number) {
    if (!Number.isFinite(now) || clip.sequence.length === 0 ||
        clip.sequence.length > 64 || clip.sequence.some((entry) =>
          !Number.isSafeInteger(entry.durationMs) || entry.durationMs < 16 ||
          entry.durationMs > 10_000) || (clip.loopRange &&
          (!clip.loop || !Number.isSafeInteger(clip.loopRange.start) ||
            !Number.isSafeInteger(clip.loopRange.end) || clip.loopRange.start < 0 ||
            clip.loopRange.end <= clip.loopRange.start || clip.loopRange.end > clip.sequence.length)))
      throw Error("动画序列或时钟无效");
    this.clip = { ...clip, loopRange: clip.loopRange && { ...clip.loopRange },
      sequence: clip.sequence.map((entry) => ({ ...entry })) };
    this.started = this.previous = now;
    return this.read(now);
  }

  read(now: number): KeyframeSample {
    const clip = this.clip;
    if (!clip) throw Error("动画尚未开始");
    if (!Number.isFinite(now) || now < this.previous) {
      this.stop();
      return { frame: clip.fallback, nextInMs: null };
    }
    this.previous = now;
    const duration = clip.sequence.reduce((sum, entry) => sum + entry.durationMs, 0);
    const elapsed = now - this.started;
    if (!clip.loop && elapsed >= duration) {
      this.stop();
      return { frame: clip.fallback, nextInMs: null };
    }
    // A delayed callback samples the current phase. It never queues missed frames.
    const range = clip.loopRange;
    const entryDuration = range
      ? clip.sequence.slice(0, range.start).reduce((sum, entry) => sum + entry.durationMs, 0) : 0;
    const bodyDuration = range
      ? clip.sequence.slice(range.start, range.end).reduce((sum, entry) => sum + entry.durationMs, 0) : duration;
    let phase = clip.loop
      ? elapsed < entryDuration ? elapsed : entryDuration + (elapsed - entryDuration) % bodyDuration
      : elapsed;
    for (const entry of clip.sequence) {
      if (phase < entry.durationMs)
        return { frame: entry.frame, nextInMs: Math.max(1, entry.durationMs - phase) };
      phase -= entry.durationMs;
    }
    return { frame: clip.fallback, nextInMs: null };
  }

  stop() { this.clip = null; }
}
