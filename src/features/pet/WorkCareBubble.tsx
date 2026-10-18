import { useEffect, useRef, useState } from "react";
import type { PetPresentation } from "../../shared/contracts";
import { Icon } from "../../shared/UI";
import type { CareBubble } from "./workCareController";
import { WorkCareSession } from "./workCareSession";

export function WorkCareBubble({
  presentation,
  ready,
  obscured,
  manualRequest,
  onOpenPanel,
}: {
  presentation: PetPresentation | null;
  ready: boolean;
  obscured: boolean;
  manualRequest: number;
  onOpenPanel: () => void;
}) {
  const [bubble, setBubble] = useState<CareBubble | null>(null);
  const session = useRef<WorkCareSession | null>(null);
  useEffect(() => {
    const owner = new WorkCareSession({
      window,
      document,
      now: () => Date.now(),
      setTimeout: (callback, delay) => window.setTimeout(callback, delay),
      clearTimeout: (id) => window.clearTimeout(id),
      change: setBubble,
    });
    session.current = owner;
    return () => {
      session.current = null;
      owner.dispose();
    };
  }, []);
  useEffect(
    () => session.current?.update(presentation, ready, obscured),
    [presentation, ready, obscured],
  );
  useEffect(() => {
    if (manualRequest > 0) session.current?.tap();
  }, [manualRequest]);
  if (!bubble || obscured) return null;
  return (
    <aside
      className="pet-care-bubble"
      role="status"
      aria-live="polite"
      data-care-kind={bubble.kind}
      data-care-tone={bubble.tone}
    >
      <button
        className="care-close"
        aria-label="关闭关心语"
        onClick={() => session.current?.dismiss()}
      >
        <Icon name="close" size={13} />
      </button>
      <p>{bubble.text}</p>
      <button
        className="care-panel-link"
        onClick={() => {
          session.current?.dismiss();
          onOpenPanel();
        }}
      >
        打开主面板 <Icon name="arrow" size={12} />
      </button>
    </aside>
  );
}
