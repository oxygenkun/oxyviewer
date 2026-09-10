import type { CSSProperties } from "react";
import type { PreviewGeometry } from "../types";
import type { Size } from "./loupe";

export interface DisplayedPreviewSize extends Size {
  geometry?: PreviewGeometry;
}

/** Invalid or absent geometry leaves the existing presentation intact. */
export function validPreviewGeometry(
  geometry: PreviewGeometry | undefined,
  raster: Size,
): PreviewGeometry | undefined {
  if (!geometry) return undefined;
  const { displaySize: size, contentRect: rect } = geometry;
  const positive = [size.width, size.height, rect.width, rect.height, raster.width, raster.height];
  if (!positive.every((n) => Number.isSafeInteger(n) && n > 0)
    || ![rect.x, rect.y].every((n) => Number.isSafeInteger(n) && n >= 0)
    || rect.x + rect.width > raster.width
    || rect.y + rect.height > raster.height) return undefined;
  return geometry;
}

export function previewContentStyles(geometry: PreviewGeometry, raster: Size): {
  frame: CSSProperties;
  image: CSSProperties;
} {
  const { displaySize, contentRect: rect } = geometry;
  const ratio = displaySize.width / displaySize.height;
  return {
    frame: {
      width: `min(100%, ${ratio * 100}cqh)`,
      height: `min(100%, ${100 / ratio}cqw)`,
    },
    image: {
      width: `${raster.width / rect.width * 100}%`,
      height: `${raster.height / rect.height * 100}%`,
      left: `${-rect.x / rect.width * 100}%`,
      top: `${-rect.y / rect.height * 100}%`,
      objectFit: "fill",
    },
  };
}
