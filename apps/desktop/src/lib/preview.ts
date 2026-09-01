import type { AssetKind } from "../types";

export const THUMBNAIL_PREVIEW_SIZE = 512;
export const LOUPE_PREVIEW_SIZE = 4_096;

/**
 * A preview stage in the progressive pipeline. Numeric stages carry the
 * `maxSize` sent to the backend; `"full"` requests a full-resolution decode
 * (mode `fullDetail`). Only formats whose full-resolution output is a single
 * image (RAW, future raster decoders) use the `"full"` stage — HEIF's full
 * resolution is streamed by `HeifTileCanvas`, so HEIF keeps only the 512px
 * placeholder here and skips the redundant 4096px JPEG upgrade.
 */
export type PreviewStageSize = number | "full";

/**
 * Progressive stages for an asset, in increasing quality order.
 *
 * - Grid/list/filmstrip (non-loupe): always `[512]`.
 * - HEIF loupe: `[512]` — the tile canvas replaces the placeholder directly.
 * - RAW loupe: `[512, 4096, "full"]` — three-tier upgrade ending in a
 *   full-resolution JPEG developed by LibRaw.
 * - Any future decodable format loupe: defaults to `[512, 4096]` until a
 *   full-resolution path is registered.
 */
export function previewStages(kind: AssetKind, large: boolean): PreviewStageSize[] {
  if (!large) return [THUMBNAIL_PREVIEW_SIZE];
  if (kind === "heif") return [THUMBNAIL_PREVIEW_SIZE];
  if (kind === "raw") return [THUMBNAIL_PREVIEW_SIZE, LOUPE_PREVIEW_SIZE, "full"];
  return [THUMBNAIL_PREVIEW_SIZE, LOUPE_PREVIEW_SIZE];
}
