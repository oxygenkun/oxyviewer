import type { FocusInfo } from "@/types";
import type { Point, Size } from "@/lib/preview/loupe";

export interface MappedFocusRegion {
  left: number;
  top: number;
  width: number;
  height: number;
  syntheticFrame: boolean;
}

/** Center of the primary focus region, used as the zoom anchor when present. */
export function focusRegionAnchor(regions: readonly MappedFocusRegion[]): Point | undefined {
  const region = regions[0];
  return region
    ? { x: region.left + region.width / 2, y: region.top + region.height / 2 }
    : undefined;
}

interface CropRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

const ASPECT_EPSILON = 0.005;
const FALLBACK_FRAME_WIDTH = 0.04;
const FALLBACK_FRAME_HEIGHT = 0.06;

/**
 * Maps camera focus coordinates into the decoded pixels currently displayed.
 *
 * Sony FocusLocation coordinates describe their own image-sized frame. RAW
 * embedded previews may instead contain an in-camera 16:9, 4:3, or 1:1 crop.
 * A developed RAW matching the full image dimensions is treated as containing
 * pixels around the camera crop; otherwise a differing embedded preview uses
 * a conservative centered-crop fallback. A point outside an inferred crop is
 * omitted rather than drawn at a misleading edge.
 */
export function mapFocusRegions(
  focus: FocusInfo | undefined,
  preview: Size | undefined,
  fullImage?: Size,
): MappedFocusRegion[] {
  if (
    !focus
    || !preview
    || focus.coordinateWidth <= 0
    || focus.coordinateHeight <= 0
    || preview.width <= 0
    || preview.height <= 0
  ) return [];

  const crop = resolvePreviewCoordinateRect(
    { width: focus.coordinateWidth, height: focus.coordinateHeight },
    preview,
    fullImage,
  );

  return focus.regions.flatMap((region) => {
    if (
      region.centerX < crop.left
      || region.centerX > crop.left + crop.width
      || region.centerY < crop.top
      || region.centerY > crop.top + crop.height
    ) return [];

    const regionWidth = region.width ?? focus.coordinateWidth * FALLBACK_FRAME_WIDTH;
    const regionHeight = region.height ?? focus.coordinateHeight * FALLBACK_FRAME_HEIGHT;
    const centerX = (region.centerX - crop.left) / crop.width;
    const centerY = (region.centerY - crop.top) / crop.height;
    const halfWidth = regionWidth / crop.width / 2;
    const halfHeight = regionHeight / crop.height / 2;
    const left = clamp01(centerX - halfWidth);
    const top = clamp01(centerY - halfHeight);
    const right = clamp01(centerX + halfWidth);
    const bottom = clamp01(centerY + halfHeight);

    return [{
      left,
      top,
      width: right - left,
      height: bottom - top,
      syntheticFrame: region.width === undefined || region.height === undefined,
    }];
  });
}

export function inferPreviewCrop(source: Size, preview: Size): CropRect {
  return centeredCoordinateRect(source, preview, false);
}

export function resolvePreviewCoordinateRect(
  source: Size,
  preview: Size,
  fullImage?: Size,
): CropRect {
  const sourceAspect = source.width / source.height;
  const previewAspect = preview.width / preview.height;
  if (aspectMatches(sourceAspect, previewAspect)) {
    return { left: 0, top: 0, width: source.width, height: source.height };
  }

  // A developed RAW can reveal pixels outside an in-camera 16:9/1:1 crop.
  // LibRaw's full output dimensions let us distinguish that expansion from an
  // embedded preview that is itself cropped. Negative offsets describe the
  // extra centered pixels around the camera focus-coordinate frame.
  const previewIsFullImage = fullImage
    && aspectMatches(previewAspect, fullImage.width / fullImage.height);
  return centeredCoordinateRect(source, preview, Boolean(previewIsFullImage));
}

function centeredCoordinateRect(source: Size, preview: Size, expand: boolean): CropRect {
  const sourceAspect = source.width / source.height;
  const previewAspect = preview.width / preview.height;
  if (sourceAspect > previewAspect) {
    if (expand) {
      const height = source.width / previewAspect;
      return { left: 0, top: (source.height - height) / 2, width: source.width, height };
    }
    const width = source.height * previewAspect;
    return { left: (source.width - width) / 2, top: 0, width, height: source.height };
  }
  if (expand) {
    const width = source.height * previewAspect;
    return { left: (source.width - width) / 2, top: 0, width, height: source.height };
  }
  const height = source.width / previewAspect;
  return { left: 0, top: (source.height - height) / 2, width: source.width, height };
}

function aspectMatches(left: number, right: number) {
  return Math.abs(left / right - 1) <= ASPECT_EPSILON;
}

function clamp01(value: number) {
  return Math.min(1, Math.max(0, value));
}
