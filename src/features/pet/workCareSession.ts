import type { PetPresentation } from "../../shared/contracts";
import { WorkCareController, type CareBubble } from "./workCareController";

export interface WorkCareHost {
  window: EventTarget;
  document: EventTarget & { readonly hidden: boolean };
  now(): number;
  setTimeout(callback: () => void, delay: number): number;
  clearTimeout(id: number): void;
  change(bubble: CareBubble | null): void;
}
/** Owns one finite expiry timeout and listeners; collection/animation clocks stay separate. */
export class WorkCareSession {
  private controller = new WorkCareController();
  private snapshot: PetPresentation | null = null;
  private ready = false;
  private visible: boolean;
  private paused = false;
  private obscured = false;
  private disposed = false;
  private timer: number | null = null;
  private value: CareBubble | null = null;
  private removers: (() => void)[] = [];
  constructor(private host: WorkCareHost) {
    this.visible = !host.document.hidden;
    const on = (
      target: EventTarget,
      name: string,
      callback: (event: Event) => void,
    ) => {
      target.addEventListener(name, callback);
      this.removers.push(() => target.removeEventListener(name, callback));
    };
    on(host.document, "visibilitychange", () => {
      this.visible = !host.document.hidden;
      this.observe();
    });
    for (const name of ["pet-visibility", "view-visibility"])
      on(host.window, name, (event) => {
        const detail = (event as CustomEvent).detail;
        this.visible =
          typeof detail === "boolean" ? detail : detail?.visible !== false;
        this.observe();
      });
    on(host.window, "desktop-runtime-state", (event) => {
      this.paused = (event as CustomEvent).detail?.paused === true;
      this.observe();
    });
    on(host.window, "initial-view-state", (event) => {
      const detail = (event as CustomEvent).detail;
      this.visible = detail?.visible !== false;
      this.paused = detail?.paused === true;
      this.observe();
    });
    on(host.window, "pet-interaction", () => this.dismiss());
  }
  update(snapshot: PetPresentation | null, ready: boolean, obscured = false) {
    if (this.disposed) return;
    const fresh = snapshot !== this.snapshot;
    this.snapshot = snapshot;
    this.ready = ready;
    this.obscured = obscured;
    this.observe(fresh);
  }
  private blocked() {
    return (
      !this.visible ||
      this.obscured ||
      this.host.document.hidden ||
      this.paused ||
      this.snapshot?.settings.petVisible === false ||
      !!(
        this.snapshot?.settings.effectiveDoNotDisturb ??
        this.snapshot?.settings.doNotDisturb
      )
    );
  }
  private observe(fresh = false) {
    if (this.disposed) return;
    if (!this.snapshot) {
      this.controller.suspend();
      this.publish(null);
      return;
    }
    if (!this.ready || this.blocked()) {
      this.publish(this.controller.observe(this.snapshot, false, true, this.host.now()));
      return;
    }
    // A visibility/popover event is not a new observation of retained facts.
    if (!fresh) {
      this.publish(this.controller.current(this.host.now()));
      return;
    }
    this.publish(
      this.controller.observe(
        this.snapshot,
        this.ready,
        this.blocked(),
        this.host.now(),
      ),
    );
  }
  private publish(bubble: CareBubble | null) {
    if (this.disposed || this.value?.id === bubble?.id) return;
    if (this.timer !== null) {
      this.host.clearTimeout(this.timer);
      this.timer = null;
    }
    this.value = bubble;
    this.host.change(bubble);
    if (bubble)
      this.timer = this.host.setTimeout(
        () => {
          this.timer = null;
          this.publish(this.controller.current(this.host.now()));
        },
        Math.max(0, bubble.expiresAt - this.host.now()),
      );
  }
  dismiss() {
    if (this.disposed) return;
    this.controller.dismiss();
    this.publish(null);
  }
  tap() {
    if (!this.disposed && this.snapshot)
      this.publish(
        this.controller.tap(
          this.snapshot.settings,
          this.ready && !this.blocked(),
          this.host.now(),
        ),
      );
  }
  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.removers.splice(0).forEach((remove) => remove());
    if (this.timer !== null) this.host.clearTimeout(this.timer);
    this.timer = null;
    this.controller.destroy();
    this.value = null;
  }
}
