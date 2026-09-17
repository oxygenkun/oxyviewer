import { describe, expect, it } from "vitest";
import { previewContentStyles, validPreviewGeometry } from "./previewGeometry";
import { resolveLoupeSourceSize } from "./loupe";
import { mapFocusRegions } from "@/lib/ui/focusArea";
import type { FocusInfo } from "@/types";

const raster = { width: 120, height: 160 };
const geometry = {
  displaySize: { width: 4672, height: 7008 },
  contentRect: { x: 7, y: 0, width: 106, height: 160 },
};

describe("preview content geometry", () => {
  it("removes padding and maps content to the full canvas despite integer rounding", () => {
    const styles = previewContentStyles(geometry, raster);
    expect(parseFloat(styles.image.width as string)).toBeCloseTo(120 / 106 * 100);
    expect(parseFloat(styles.image.left as string)).toBeCloseTo(-7 / 106 * 100);
    expect(styles.image.height).toBe("100%");
    expect(styles.image.objectFit).toBe("fill");
    const logicalWidth = 400;
    const scale = logicalWidth / geometry.contentRect.width;
    expect(-7 * scale + geometry.contentRect.x * scale).toBe(0);
    expect(-7 * scale + (7 + 106) * scale).toBeCloseTo(logicalWidth);
  });

  it("rejects invalid geometry instead of moving or hiding image content", () => {
    expect(validPreviewGeometry(geometry, raster)).toBe(geometry);
    expect(validPreviewGeometry(undefined, raster)).toBeUndefined();
    for (const rect of [
      { ...geometry.contentRect, x: -1 },
      { ...geometry.contentRect, width: 0 },
      { ...geometry.contentRect, x: 15 },
      { ...geometry.contentRect, height: Infinity },
      { ...geometry.contentRect, y: 0.5 },
    ]) expect(validPreviewGeometry({ ...geometry, contentRect: rect }, raster)).toBeUndefined();
    expect(validPreviewGeometry({ ...geometry, displaySize: { width: 0, height: 7008 } }, raster)).toBeUndefined();
  });

  it("keeps focus coordinates stable when padded preview is replaced by full", () => {
    const focus = {
      coordinateWidth: 4672, coordinateHeight: 7008,
      regions: [{ centerX: 1500, centerY: 1800, width: 200, height: 300 }],
    } as FocusInfo;
    const full = mapFocusRegions(focus, geometry.displaySize, geometry.displaySize);
    expect(mapFocusRegions(focus, raster, geometry.displaySize)).not.toEqual(full);
    for (const metadata of [undefined, { width: 120, height: 160 }, geometry.displaySize]) {
      const previewCanvas = resolveLoupeSourceSize("heif", raster, undefined, metadata, { width: 3, height: 2 }, geometry);
      const fullCanvas = resolveLoupeSourceSize("heif", raster, geometry.displaySize, metadata, { width: 3, height: 2 }, geometry);
      expect(previewCanvas).toEqual(fullCanvas);
      expect(mapFocusRegions(focus, previewCanvas, metadata)).toEqual(full);
    }
  });
});
