import { useLayoutEffect, useRef } from "react";
import type { ThumbnailOrientation } from "../types";

/**
 * Runs `retain` on the layout pass that switches thumbnail orientation.
 *
 * Landscape- and portrait-first thumbnails use different metrics, so the same
 * scroll offset can move the active photo outside its viewport. Callers pass a
 * callback that re-measures its virtualizer and scrolls the active photo back
 * into view. The mount render is skipped so the existing folder-restore
 * behavior keeps owning first paint.
 */
export function useOrientationRetention(
  orientation: ThumbnailOrientation,
  retain: () => void,
): void {
  const previousOrientation = useRef(orientation);
  useLayoutEffect(() => {
    if (previousOrientation.current === orientation) return;
    previousOrientation.current = orientation;
    retain();
  }, [orientation, retain]);
}
