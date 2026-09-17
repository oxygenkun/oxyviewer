export function horizontalWheelDelta(
  event: Pick<WheelEvent, "deltaX" | "deltaY" | "deltaMode">,
  lineHeight: number,
  pageWidth: number,
): number {
  const delta = event.deltaX || event.deltaY;
  return delta * (event.deltaMode === 1 ? lineHeight : event.deltaMode === 2 ? pageWidth : 1);
}

/** A cancellable native listener avoids React's passive wheel delegation. */
export function installFilmstripWheel(element: HTMLElement): () => void {
  let frame: number | undefined;
  let pending = 0;
  const lineHeight = parseFloat(getComputedStyle(element).lineHeight) || 16;
  const wheel = (event: WheelEvent) => {
    const delta = horizontalWheelDelta(event, lineHeight, element.clientWidth);
    if (!delta || event.ctrlKey) return;
    event.preventDefault();
    pending += delta;
    if (frame !== undefined) return;
    frame = requestAnimationFrame(() => {
      frame = undefined;
      element.scrollLeft += pending;
      pending = 0;
    });
  };
  element.addEventListener("wheel", wheel, { passive: false });
  return () => {
    element.removeEventListener("wheel", wheel);
    if (frame !== undefined) cancelAnimationFrame(frame);
  };
}
