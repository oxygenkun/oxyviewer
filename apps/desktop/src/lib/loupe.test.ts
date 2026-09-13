import { describe, expect, it } from "vitest";
import {
  clampPan,
  clampZoom,
  filmstripItemWidth,
  fitSize,
  getNavigatorViewport,
  MAX_PIXEL_ZOOM_PERCENT,
  nextCycleZoom,
  orderVisibleFilmstripItems,
  panByNavigatorDelta,
  panFromNavigatorPoint,
  pixelZoomPercent,
  resolveLoupeSourceSize,
  shouldFetchFilmstripPage,
  zoomAtPoint,
  zoomForPixelPercent,
} from "./loupe";

const stage = { width: 800, height: 600 };
const image = { width: 700, height: 500 };

describe("loupe geometry", () => {
  it("reserves the full filmstrip width for unloaded assets", () => {
    expect(filmstripItemWidth(116)).toBe(129);
    expect(filmstripItemWidth(180)).toBe(209);
    expect(filmstripItemWidth(300)).toBe(359);
    expect(filmstripItemWidth(116, "portrait")).toBe(70);
    expect(filmstripItemWidth(180, "portrait")).toBe(114);
    expect(filmstripItemWidth(300, "portrait")).toBe(195);
  });

  it("prioritizes visible filmstrip items from the viewport center", () => {
    expect(orderVisibleFilmstripItems([
      { id: "off-left", start: -120, end: -20 },
      { id: "left", start: 0, end: 80 },
      { id: "center", start: 90, end: 170 },
      { id: "right", start: 180, end: 260 },
      { id: "off-right", start: 270, end: 350 },
    ], 0, 260)).toEqual(["center", "left", "right"]);
  });

  it("loads sequential pages until they cover a jumped virtual viewport", () => {
    expect(shouldFetchFilmstripPage(510, 250, true, false)).toBe(true);
    expect(shouldFetchFilmstripPage(510, 500, true, false)).toBe(true);
    expect(shouldFetchFilmstripPage(510, 750, true, false)).toBe(false);
    expect(shouldFetchFilmstripPage(510, 500, true, true)).toBe(false);
    expect(shouldFetchFilmstripPage(510, 500, false, false)).toBe(false);
  });

  it("uses HEIF full-resolution dimensions instead of the 512 px placeholder", () => {
    expect(resolveLoupeSourceSize(
      "heif",
      { width: 512, height: 341 },
      { width: 7_008, height: 4_672 },
      undefined,
      { width: 3, height: 2 },
    )).toEqual({ width: 7_008, height: 4_672 });
  });

  it("fits landscape and portrait images without changing their aspect ratio", () => {
    expect(fitSize({ width: 800, height: 600 }, { width: 1_600, height: 900 }))
      .toEqual({ width: 800, height: 450 });
    expect(fitSize({ width: 800, height: 600 }, { width: 900, height: 1_600 }))
      .toEqual({ width: 337.5, height: 600 });
  });

  it("reports zoom as displayed pixels relative to source pixels", () => {
    expect(pixelZoomPercent({ width: 1_000, height: 667 }, { width: 6_000, height: 4_000 }, 3))
      .toBe(50);
  });

  it("converts an exact displayed-pixel percentage to zoom and clamps it", () => {
    const fitted = { width: 1_000, height: 667 };
    const source = { width: 6_000, height: 4_000 };
    const maxZoom = zoomForPixelPercent(fitted, source, MAX_PIXEL_ZOOM_PERCENT);

    expect(zoomForPixelPercent(fitted, source, 125)).toBe(7.5);
    expect(clampZoom(zoomForPixelPercent(fitted, source, 500), maxZoom)).toBe(24);
    expect(clampZoom(2, 0.5)).toBe(1);
  });

  it("cycles fit → 20% → 100% → fit", () => {
    const fitted = { width: 1_000, height: 667 };
    const source = { width: 6_000, height: 4_000 };
    const maxZoom = zoomForPixelPercent(fitted, source, MAX_PIXEL_ZOOM_PERCENT);

    const twenty = zoomForPixelPercent(fitted, source, 20);
    const hundred = zoomForPixelPercent(fitted, source, 100);
    expect(nextCycleZoom(1, fitted, source, maxZoom)).toBe(twenty);
    expect(nextCycleZoom(twenty, fitted, source, maxZoom)).toBe(hundred);
    expect(nextCycleZoom(hundred, fitted, source, maxZoom)).toBe(1);
  });

  it("resumes the cycle from a manual zoom level", () => {
    const fitted = { width: 1_000, height: 667 };
    const source = { width: 6_000, height: 4_000 };
    const maxZoom = zoomForPixelPercent(fitted, source, MAX_PIXEL_ZOOM_PERCENT);

    expect(nextCycleZoom(3, fitted, source, maxZoom))
      .toBe(zoomForPixelPercent(fitted, source, 100));
    expect(nextCycleZoom(maxZoom, fitted, source, maxZoom)).toBe(1);
  });

  it("skips cycle steps that would not magnify a small source", () => {
    const fitted = { width: 1_000, height: 667 };
    const source = { width: 900, height: 600 };
    const maxZoom = zoomForPixelPercent(fitted, source, MAX_PIXEL_ZOOM_PERCENT);

    expect(nextCycleZoom(1, fitted, source, maxZoom)).toBe(1);
  });

  it("clamps panning to the visible image bounds", () => {
    expect(clampPan({ x: 999, y: -999 }, 2, stage, image)).toEqual({ x: 300, y: -200 });
    expect(clampPan({ x: 80, y: 80 }, 1, stage, image)).toEqual({ x: 0, y: 0 });
  });

  it("keeps the point below the cursor fixed while zooming", () => {
    expect(zoomAtPoint(1, 2, { x: 0, y: 0 }, { x: 100, y: 50 }, stage, image))
      .toEqual({ x: -100, y: -50 });
  });

  it("maps pan offsets to the navigator viewport and back", () => {
    const offset = panFromNavigatorPoint({ x: 0.25, y: 0.25 }, 2, stage, image);
    expect(offset).toEqual({ x: 300, y: 200 });
    expect(getNavigatorViewport(2, offset, stage, image)).toEqual({
      left: 0,
      top: 0,
      width: 800 / 1_400,
      height: 0.6,
    });
  });

  it("drags the navigator incrementally and discards movement into a boundary", () => {
    const navigator = { width: 140, height: 100 };
    const atRightEdge = { x: -300, y: 0 };

    expect(panByNavigatorDelta(atRightEdge, { x: 20, y: 0 }, 2, stage, image, navigator))
      .toEqual(atRightEdge);
    expect(panByNavigatorDelta(atRightEdge, { x: -10, y: 0 }, 2, stage, image, navigator))
      .toEqual({ x: -200, y: 0 });
  });
});
