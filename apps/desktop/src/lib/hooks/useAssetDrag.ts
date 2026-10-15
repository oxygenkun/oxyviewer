import { useRef, type DragEvent, type MouseEvent, type PointerEvent } from "react";
import { isTauri } from "@/lib/api";
import type { AssetSummary } from "@/types";

const DRAG_DISTANCE = 6;

/** Keep pointer motion out of React state and let the OS own an active drag. */
export function useAssetDrag(
  asset: AssetSummary,
  onDrag: (asset: AssetSummary) => void,
  panInstead = false,
) {
  const gesture = useRef<{ pointerId: number; x: number; y: number; asset: AssetSummary } | undefined>(undefined);
  const suppressClick = useRef(false);

  const finish = (event: PointerEvent<HTMLElement>) => {
    if (gesture.current?.pointerId !== event.pointerId) return;
    gesture.current = undefined;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };

  const suppressDragClick = (event: MouseEvent<HTMLElement>) => {
    // Keyboard activation has no pointer gesture and must remain usable even
    // if the native session consumed the previous pointerup.
    if (!suppressClick.current || event.detail === 0) return;
    event.preventDefault();
    event.stopPropagation();
  };

  return {
    draggable: false,
    onDragStart: (event: DragEvent<HTMLElement>) => event.preventDefault(),
    onPointerDown: (event: PointerEvent<HTMLElement>) => {
      suppressClick.current = false;
      if (!isTauri() || event.button !== 0 || event.isPrimary === false
        || event.pointerType === "touch" || (panInstead && !event.altKey)) return;
      const control = (event.target as HTMLElement).closest("button, input, textarea, select, [role='button']");
      if (control && control !== event.currentTarget) return;
      gesture.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, asset };
      event.currentTarget.setPointerCapture(event.pointerId);
      event.stopPropagation();
    },
    onPointerMove: (event: PointerEvent<HTMLElement>) => {
      const start = gesture.current;
      if (!start || start.pointerId !== event.pointerId) return;
      event.stopPropagation();
      if (Math.hypot(event.clientX - start.x, event.clientY - start.y) < DRAG_DISTANCE) return;
      // Native sessions can consume pointerup/cancel. Clear capture and state
      // before invoking IPC so the next click/drag cannot inherit this gesture.
      finish(event);
      suppressClick.current = true;
      event.preventDefault();
      onDrag(start.asset);
    },
    onPointerUp: finish,
    onPointerCancel: finish,
    onLostPointerCapture: finish,
    onClickCapture: suppressDragClick,
    onDoubleClickCapture: suppressDragClick,
  };
}
