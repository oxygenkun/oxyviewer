import type { AssetKind, PreviewGeometry, ThumbnailOrientation } from "../types";

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
const FILMSTRIP_VERTICAL_CHROME = 13;
export const FILMSTRIP_GAP = 5;
export function filmstripItemWidth(
  filmstripHeight: number,
  orientation: ThumbnailOrientation = "landscape",
): number {
  const itemHeight = filmstripHeight - FILMSTRIP_VERTICAL_CHROME;
  const ratio = orientation === "portrait" ? 0.68 : 1.25;
  const minWidth = orientation === "portrait" ? 48 : 90;
  return Math.max(minWidth, Math.round(itemHeight * ratio));
}

/** Fetches sequential pages until the loaded range covers the virtual viewport. */
export function shouldFetchFilmstripPage(
  lastRenderedIndex: number | undefined,
  loadedCount: number,
  hasNextPage: boolean,
  isFetchingNextPage: boolean,
): boolean {
  return lastRenderedIndex !== undefined
    && lastRenderedIndex >= loadedCount - 5
    && hasNextPage
    && !isFetchingNextPage;
}

export interface FilmstripItemRange {
  id: string;
  start: number;
  end: number;
}

/** Returns visible filmstrip items nearest the viewport center first. */
export function orderVisibleFilmstripItems(
  items: readonly FilmstripItemRange[],
  viewportStart: number,
  viewportEnd: number,
): string[] {
  const viewportCenter = (viewportStart + viewportEnd) / 2;
  return items
    .filter((item) => item.end > viewportStart && item.start < viewportEnd)
    .sort((left, right) => {
      const leftDistance = Math.abs((left.start + left.end) / 2 - viewportCenter);
      const rightDistance = Math.abs((right.start + right.end) / 2 - viewportCenter);
      return leftDistance - rightDistance || left.start - right.start;
    })
    .map((item) => item.id);
}

export function resolveLoupeSourceSize(
  kind: AssetKind,
  previewNaturalSize: Size | undefined,
  fullResolutionSize: Size | undefined,
  metadataSize: Size | undefined,
  fallback: Size,
  previewGeometry?: PreviewGeometry,
) {
  // The pixel-zoom scale is always anchored to the complete image's real
  // dimensions, never to the raster currently on screen. Intermediate
  // artifacts (retained thumbnail, scaled preview, interim embedded JPEG)
  // carry fewer pixels; letting them define the scale made the percentage
  // jump while the full image loads (a 512 px RAW thumbnail showed ~100%
  // where the settled view is ~6%). Metadata may be stored pre-rotation,
  // so match the on-screen orientation before using it.
  if (kind === "heif") {
    // HEIF's <img> is only a 512 px placeholder. The tile canvas reports the
    // real full-resolution pixels, and recognized previews (e.g. Sony HIF)
    // carry full-resolution display dimensions; raw container metadata is only
    // a fallback and may be pre-rotation, so it follows the on-screen
    // orientation of the placeholder.
    if (fullResolutionSize) return fullResolutionSize;
    if (previewGeometry?.displaySize) return previewGeometry.displaySize;
    if (metadataSize) {
      return previewNaturalSize
        ? matchDisplayOrientation(metadataSize, previewNaturalSize)
        : metadataSize;
    }
    return previewNaturalSize ?? fallback;
  }
  if (metadataSize) {
    const displayed = previewGeometry?.displaySize ?? previewNaturalSize;
    return displayed ? matchDisplayOrientation(metadataSize, displayed) : metadataSize;
  }
  return previewNaturalSize ?? fallback;
}

/** Metadata may be stored pre-rotation; match the on-screen orientation. */
function matchDisplayOrientation(reference: Size, displayed: Size): Size {
  const referencePortrait = reference.height > reference.width;
  const displayedPortrait = displayed.height > displayed.width;
  return referencePortrait === displayedPortrait
    ? reference
    : { width: reference.height, height: reference.width };
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

export const ZOOM_CYCLE_PERCENTS = [20, 100] as const;
const ZOOM_EPSILON = 0.001;

/**
 * Cycles fit → 20% → 100% → fit. Steps that would not magnify beyond the
 * fitted view (small sources) are skipped so the cycle never gets stuck.
 */
export function nextCycleZoom(
  zoom: number,
  fitted: Size,
  source: Size,
  maxZoom: number,
): number {
  const targets = ZOOM_CYCLE_PERCENTS
    .map((percent) => clampZoom(zoomForPixelPercent(fitted, source, percent), maxZoom))
    .filter((target) => target > MIN_ZOOM + ZOOM_EPSILON);
  if (zoom <= MIN_ZOOM + ZOOM_EPSILON) return targets[0] ?? MIN_ZOOM;
  return targets.find((target) => target > zoom + ZOOM_EPSILON) ?? MIN_ZOOM;
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
