/** BFCache keeps the same window; terminal pagehide or HMR cleanup releases it. */
export function bindPetWindowLifetime(owner: { dispose(): void }, target: EventTarget) {
  let bound = true;
  const dispose = () => {
    if (!bound) return;
    bound = false;
    target.removeEventListener("pagehide", pagehide);
    owner.dispose();
  };
  const pagehide = (event: Event) => {
    if (!(event as PageTransitionEvent).persisted) dispose();
  };
  target.addEventListener("pagehide", pagehide);
  return dispose;
}
