export interface IntervalHost {
  set(callback: () => void, milliseconds: number): number;
  clear(id: number): void;
}
const browserIntervals: IntervalHost = {
  set: (callback, ms) => window.setInterval(callback, ms),
  clear: (id) => window.clearInterval(id),
};

/** A visible view owns one polling interval. The Rust collector is independent. */
export class ViewRefreshLoop {
  private timer: number | null = null;
  private visible = false;
  private inFlight: Promise<void> | null = null;
  private destroyed = false;
  private refreshAfterCurrent = false;
  constructor(
    private refresh: () => Promise<void>,
    private intervals = browserIntervals,
    private onError: (error: unknown) => void = () => undefined,
  ) {}
  setVisible(visible: boolean): void {
    if (this.destroyed || visible === this.visible) return;
    this.visible = visible;
    if (!visible) this.refreshAfterCurrent = false;
    if (this.timer != null) {
      this.intervals.clear(this.timer);
      this.timer = null;
    }
    if (visible) {
      void this.request(true);
      this.timer = this.intervals.set(() => void this.request(), 3000);
    }
  }
  request(afterCurrent = false): Promise<void> {
    if (this.destroyed || !this.visible) return Promise.resolve();
    if (this.inFlight) {
      if (afterCurrent) this.refreshAfterCurrent = true;
      return this.inFlight;
    }
    let complete!: () => void;
    const flight = new Promise<void>((resolve) => { complete = resolve; });
    // Publish ownership before invoking refresh: even a synchronous throw or a
    // reentrant invalidation must see the same flight.
    this.inFlight = flight;
    void this.drain().then(complete, complete);
    return flight;
  }
  private async drain(): Promise<void> {
    try {
      do {
        this.refreshAfterCurrent = false;
        try {
          await this.refresh();
        } catch (error) {
          // Timers and native events intentionally do not await request(). A
          // failed read must not escape as an unhandled rejection or stop polling.
          try {
            this.onError(error);
          } catch {
            // Error reporting cannot break the visible-view lifecycle either.
          }
        }
      } while (this.refreshAfterCurrent && this.visible && !this.destroyed);
    } finally {
      this.inFlight = null;
      this.refreshAfterCurrent = false;
    }
  }
  diagnostics() {
    return {
      visible: this.visible,
      intervals: this.timer == null ? 0 : 1,
      running: this.inFlight != null,
      destroyed: this.destroyed,
    };
  }
  dispose(): void {
    this.destroyed = true;
    this.visible = false;
    this.refreshAfterCurrent = false;
    if (this.timer != null) this.intervals.clear(this.timer);
    this.timer = null;
  }
}
