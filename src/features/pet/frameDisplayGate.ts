/** Async region ACKs may settle out of order. Only the newest live frame commits. */
export function createFrameDisplayGate(beforeDisplay: ((frameKey: string) => Promise<void>) | undefined,
  live: () => boolean) {
  let sequence = 0;
  return async (frameKey: string, commit: () => void): Promise<boolean> => {
    const token = ++sequence;
    try { await beforeDisplay?.(frameKey); } catch { return false; }
    if (!live() || token !== sequence) return false;
    commit();
    return true;
  };
}
