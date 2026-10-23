export type PetPopover = "card" | "menu" | null;
type Interaction = { active: boolean; revision: number };
type Dismissal = { reason?: string; revision?: number };

export interface PetPopoverHost {
  window: EventTarget;
  document: EventTarget & { readonly hidden: boolean };
  change: (value: PetPopover) => void;
  interrupt: () => void;
  error: (error: unknown) => void;
  interaction?: (value: Interaction) => Promise<Interaction>;
  listenDismiss?: (callback: (value: Dismissal) => void) => Promise<() => void>;
}

// Native revisions also survive component remounts. A clock-based seed avoids
// resetting the sequence on WebView reload; frontend_ready supplies its watermark.
let lastRevision = 0;
function observeRevision(revision: number | undefined) {
  if (Number.isSafeInteger(revision) && revision! >= 0)
    lastRevision = Math.max(lastRevision, revision!);
}
function nextRevision() {
  lastRevision = Math.max(lastRevision + 1, Date.now() * 1000);
  return lastRevision;
}

/** Owns the transient native focus lease, never a persisted pet preference. */
export class PetPopoverController {
  private value: PetPopover = null;
  private revision = 0;
  private disposed = false;
  private removers: (() => void)[] = [];
  private settlement: Promise<boolean> = Promise.resolve(true);

  constructor(private readonly host: PetPopoverHost) {
    const on = (
      target: EventTarget,
      name: string,
      callback: (event: Event) => void,
    ) => {
      target.addEventListener(name, callback);
      this.removers.push(() => target.removeEventListener(name, callback));
    };
    on(host.window, "blur", () => this.close());
    on(host.window, "keydown", (event) => {
      if ((event as KeyboardEvent).key === "Escape" && this.value) {
        event.preventDefault();
        this.close();
      }
    });
    on(host.document, "visibilitychange", () => {
      if (host.document.hidden) this.close();
    });
    for (const event of ["pet-visibility", "view-visibility"])
      on(host.window, event, (value) => {
        if (
          (value as CustomEvent<{ visible?: boolean }>).detail?.visible ===
          false
        )
          this.close();
      });
    on(host.window, "desktop-runtime-state", (event) => {
      if ((event as CustomEvent<{ paused?: boolean }>).detail?.paused)
        this.close();
    });
    on(host.window, "initial-view-state", (event) => {
      const detail = (
        event as CustomEvent<{
          visible?: boolean;
          paused?: boolean;
          petInteractionRevision?: number;
        }>
      ).detail;
      observeRevision(detail?.petInteractionRevision);
      if (detail?.visible === false || detail?.paused) this.close();
    });
    void host
      .listenDismiss?.((event) => {
        observeRevision(event.revision);
        // An old blur may arrive after a new deliberate open. It cannot dismiss
        // the newer user interaction or cause an asynchronous reopen.
        if (event.revision != null && event.revision < this.revision) return;
        this.close();
      })
      .then((remove) => {
        if (this.disposed) remove();
        else this.removers.push(remove);
      })
      .catch((error) => {
        if (!this.disposed) {
          this.close();
          host.error(error);
        }
      });
  }

  toggle(value: Exclude<PetPopover, null>) {
    if (this.disposed) return;
    if (this.value === value) return this.close();
    this.value = value;
    this.host.interrupt();
    this.host.change(value);
    this.setInteraction(true);
  }

  close() {
    if (this.disposed || !this.value) return;
    this.value = null;
    this.host.interrupt();
    this.host.change(null);
    this.setInteraction(false);
  }

  /** 动作必须等同一代原生关闭完成，不能沿用尚未收起的弹窗几何。 */
  async closeForAction(): Promise<boolean> {
    this.close();
    const revision = this.revision;
    return await this.settlement && !this.disposed && this.value === null && revision === this.revision;
  }

  private setInteraction(active: boolean) {
    const revision = (this.revision = nextRevision());
    this.settlement = Promise.resolve(this.host
      .interaction?.({ active, revision })
    ).then((state) => {
        observeRevision(state?.revision);
        return !this.disposed && this.revision === revision && (!state || state.active === active && state.revision >= revision);
      })
      .catch((error) => {
        if (this.disposed || this.revision !== revision) return false;
        if (active) this.close();
        this.host.error(error);
        return false;
      });
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.removers.splice(0).forEach((remove) => remove());
    this.value = null;
    // A pending open is superseded even when React unmounts before IPC resolves.
    void this.host
      .interaction?.({ active: false, revision: nextRevision() })
      .catch(() => undefined);
  }
}
