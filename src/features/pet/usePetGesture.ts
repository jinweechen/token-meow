import { useRef } from "react";
import { useGesture } from "@use-gesture/react";

interface DragOrigin { clientX: number; clientY: number; viewportWidth: number; viewportHeight: number }

/** 手势识别和指针捕获交给 use-gesture，原生侧仍核验按下状态及桌面几何。 */
export function usePetGesture(options: {
  tap(): void;
  interrupt(): void;
  drag(origin: DragOrigin): void;
}) {
  const press = useRef<(DragOrigin & { pointerId: number; dragged: boolean }) | null>(null);
  return useGesture({
    onPointerDown({ event }) {
      if (event.button !== 0 || !event.isPrimary) return;
      event.preventDefault();
      press.current = { clientX: event.clientX, clientY: event.clientY,
        viewportWidth: window.innerWidth, viewportHeight: window.innerHeight,
        pointerId: event.pointerId, dragged: false };
      options.interrupt();
    },
    onDrag({ active, down, tap, canceled, event }) {
      const origin = press.current;
      if (!origin || !("pointerId" in event) || event.pointerId !== origin.pointerId) return;
      if (canceled || event.type === "pointercancel" || event.type === "lostpointercapture") {
        press.current = null;
        return;
      }
      if (down && active && !origin.dragged) {
        origin.dragged = true;
        options.drag(origin);
      }
      if (!down && event.type === "pointerup") {
        press.current = null;
        if (tap && !origin.dragged) options.tap();
      }
    },
    onPointerCancel() { press.current = null; },
    onLostPointerCapture() { press.current = null; },
  }, { drag: { threshold: 5, filterTaps: true, tapsThreshold: 5, triggerAllEvents: true,
    pointer: { buttons: 1, keys: false, capture: true } } });
}
