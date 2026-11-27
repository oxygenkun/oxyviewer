import type { Point } from "./loupe";

const WHEEL_ZOOM_RATE = 0.002;
const LOUPE_OVERLAYS = ".loupe__controls, .loupe__navigator, .loupe__settings";

interface LoupeInputPreferences {
  loupeWheelZoom: boolean;
  loupeZoomSensitivity: number;
}

export function wheelZoomRate(deltaMode: number, sensitivity: number): number {
  return deltaMode === WheelEvent.DOM_DELTA_PIXEL ? WHEEL_ZOOM_RATE * sensitivity / 100 : WHEEL_ZOOM_RATE;
}

export function listenLoupeInput(
  stage: HTMLElement,
  getPreferences: () => LoupeInputPreferences,
  zoomBy: (factor: number, point: Point) => void,
  panBy: (delta: Point) => void,
): () => void {
  let previousScale: number | undefined;
  const acceptsInput = (event: Event) => !event.defaultPrevented
    && !(event.target instanceof Element && event.target.closest(LOUPE_OVERLAYS));

  const zoomAtCursor = (factor: number, event: MouseEvent) => {
    const rect = stage.getBoundingClientRect();
    zoomBy(factor, {
      x: Number.isFinite(event.clientX) ? event.clientX - rect.left - rect.width / 2 : 0,
      y: Number.isFinite(event.clientY) ? event.clientY - rect.top - rect.height / 2 : 0,
    });
  };

  const wheel = (event: WheelEvent) => {
    if (!acceptsInput(event)) return;
    event.preventDefault();
    const deltaMode = event.deltaMode;
    const { loupeWheelZoom, loupeZoomSensitivity } = getPreferences();
    // Smooth scrolling and high-resolution mouse wheels can both report pixel
    // deltas. A wheel event with Ctrl also represents pinch in some WebViews.
    const unmodifiedPixelScroll = deltaMode === WheelEvent.DOM_DELTA_PIXEL
      && !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey;
    if (unmodifiedPixelScroll && (!loupeWheelZoom || event.deltaY === 0)) {
      if (event.deltaX || event.deltaY) panBy({ x: -event.deltaX, y: -event.deltaY });
      return;
    }
    const rate = wheelZoomRate(deltaMode, loupeZoomSensitivity);
    if (event.deltaY !== 0) zoomAtCursor(Math.exp(-event.deltaY * rate), event);
  };
  const gestureStart = (event: Event) => {
    previousScale = undefined;
    if (!acceptsInput(event)) return;
    event.preventDefault();
    previousScale = 1;
  };
  const gestureChange = (event: Event) => {
    if (previousScale === undefined || event.defaultPrevented) return;
    event.preventDefault();
    // WebKit reports scale relative to the start of the pinch.
    const gesture = event as MouseEvent & { scale: number };
    if (!Number.isFinite(gesture.scale) || gesture.scale <= 0) return;
    const factor = (gesture.scale / previousScale) ** (getPreferences().loupeZoomSensitivity / 100);
    previousScale = gesture.scale;
    if (factor !== 1) zoomAtCursor(factor, gesture);
  };
  const gestureEnd = (event: Event) => {
    if (previousScale === undefined) return;
    event.preventDefault();
    previousScale = undefined;
  };

  stage.addEventListener("wheel", wheel, { passive: false });
  stage.addEventListener("gesturestart", gestureStart);
  stage.addEventListener("gesturechange", gestureChange);
  stage.addEventListener("gestureend", gestureEnd);
  return () => {
    stage.removeEventListener("wheel", wheel);
    stage.removeEventListener("gesturestart", gestureStart);
    stage.removeEventListener("gesturechange", gestureChange);
    stage.removeEventListener("gestureend", gestureEnd);
  };
}
