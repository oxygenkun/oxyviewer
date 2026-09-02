import type { AssetKind } from "../types";

export interface Point {
  x: number;
  y: number;
}

export interface Size {
  width: number;
  height: number;
}

export interface NavigatorViewport {
  left: number;
  top: number;
  width: number;
  height: number;
}

export const MIN_ZOOM = 1;
export const MAX_PIXEL_ZOOM_PERCENT = 400;

export function resolveLoupeSourceSize(
  kind: AssetKind,
  previewNaturalSize: Size | undefined,
  fullResolutionSize: Size | undefined,
  metadataSize: Size | undefined,
  fallback: Size,
) {
  // HEIF's <img> is only a 512 px placeholder. It must never define the
  // pixel-zoom scale once the full-resolution tile canvas is available.
  if (kind === "heif") {
    return fullResolutionSize ?? metadataSize ?? previewNaturalSize ?? fallback;
  }
  return previewNaturalSize ?? metadataSize ?? fallback;
}

export function clampZoom(zoom: number, maxZoom = MAX_PIXEL_ZOOM_PERCENT) {
  return Math.min(Math.max(MIN_ZOOM, maxZoom), Math.max(MIN_ZOOM, zoom));
}

export function fitSize(container: Size, source: Size): Size {
  if (container.width <= 0 || container.height <= 0 || source.width <= 0 || source.height <= 0) {
    return { width: 0, height: 0 };
  }
  const scale = Math.min(container.width / source.width, container.height / source.height);
  return {
    width: source.width * scale,
    height: source.height * scale,
  };
}

export function pixelZoomPercent(fitted: Size, source: Size, zoom: number) {
  if (fitted.width <= 0 || source.width <= 0) return 0;
  return Math.round((fitted.width * zoom / source.width) * 100);
}

export function zoomForPixelPercent(fitted: Size, source: Size, percent: number) {
  if (fitted.width <= 0 || source.width <= 0) return MIN_ZOOM;
  return (percent / 100) * (source.width / fitted.width);
}

export function clampPan(offset: Point, zoom: number, stage: Size, image: Size): Point {
  const maxX = Math.max(0, (image.width * zoom - stage.width) / 2);
  const maxY = Math.max(0, (image.height * zoom - stage.height) / 2);
  return {
    x: Math.min(maxX, Math.max(-maxX, offset.x)),
    y: Math.min(maxY, Math.max(-maxY, offset.y)),
  };
}

export function zoomAtPoint(
  zoom: number,
  nextZoom: number,
  offset: Point,
  pointFromStageCenter: Point,
  stage: Size,
  image: Size,
): Point {
  const ratio = nextZoom / zoom;
  return clampPan({
    x: pointFromStageCenter.x - (pointFromStageCenter.x - offset.x) * ratio,
    y: pointFromStageCenter.y - (pointFromStageCenter.y - offset.y) * ratio,
  }, nextZoom, stage, image);
}

export function getNavigatorViewport(
  zoom: number,
  offset: Point,
  stage: Size,
  image: Size,
): NavigatorViewport {
  const width = Math.min(1, stage.width / (image.width * zoom));
  const height = Math.min(1, stage.height / (image.height * zoom));
  const centerX = 0.5 - offset.x / (image.width * zoom);
  const centerY = 0.5 - offset.y / (image.height * zoom);
  return {
    left: Math.min(1 - width, Math.max(0, centerX - width / 2)),
    top: Math.min(1 - height, Math.max(0, centerY - height / 2)),
    width,
    height,
  };
}

export function panFromNavigatorPoint(
  normalizedPoint: Point,
  zoom: number,
  stage: Size,
  image: Size,
): Point {
  return clampPan({
    x: (0.5 - normalizedPoint.x) * image.width * zoom,
    y: (0.5 - normalizedPoint.y) * image.height * zoom,
  }, zoom, stage, image);
}

export function panByNavigatorDelta(
  offset: Point,
  pointerDelta: Point,
  zoom: number,
  stage: Size,
  image: Size,
  navigator: Size,
): Point {
  if (navigator.width <= 0 || navigator.height <= 0) return offset;
  return clampPan({
    x: offset.x - (pointerDelta.x / navigator.width) * image.width * zoom,
    y: offset.y - (pointerDelta.y / navigator.height) * image.height * zoom,
  }, zoom, stage, image);
}
