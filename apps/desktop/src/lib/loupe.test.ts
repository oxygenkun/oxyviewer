import { describe, expect, it } from "vitest";
import {
  clampPan,
  fitSize,
  getNavigatorViewport,
  panFromNavigatorPoint,
  pixelZoomPercent,
  zoomAtPoint,
} from "./loupe";

const stage = { width: 800, height: 600 };
const image = { width: 700, height: 500 };

describe("loupe geometry", () => {
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
});
