import { describe, expect, it } from "vitest";
import { inferPreviewCrop, mapFocusRegions, resolvePreviewCoordinateRect } from "./focusArea";
import type { FocusInfo } from "../types";

const focus = (centerX: number, centerY: number): FocusInfo => ({
  coordinateWidth: 6_000,
  coordinateHeight: 4_000,
  regions: [{ centerX, centerY }],
});

describe("focus area mapping", () => {
  it("maps matching-aspect coordinates directly", () => {
    const [region] = mapFocusRegions(focus(1_500, 1_000), { width: 1_200, height: 800 });
    expect(region.left + region.width / 2).toBeCloseTo(0.25);
    expect(region.top + region.height / 2).toBeCloseTo(0.25);
    expect(region.syntheticFrame).toBe(true);
  });

  it("maps through a centered wide preview crop", () => {
    const crop = inferPreviewCrop(
      { width: 6_000, height: 4_000 },
      { width: 1_600, height: 900 },
    );
    expect(crop).toEqual({ left: 0, top: 312.5, width: 6_000, height: 3_375 });

    const [region] = mapFocusRegions(focus(3_000, 2_000), { width: 1_600, height: 900 });
    expect(region.left + region.width / 2).toBeCloseTo(0.5);
    expect(region.top + region.height / 2).toBeCloseTo(0.5);
  });

  it("expands a camera crop when the displayed image matches the full RAW", () => {
    expect(resolvePreviewCoordinateRect(
      { width: 6_000, height: 3_375 },
      { width: 6_000, height: 4_000 },
      { width: 6_000, height: 4_000 },
    )).toEqual({ left: 0, top: -312.5, width: 6_000, height: 4_000 });

    const [region] = mapFocusRegions({
      coordinateWidth: 6_000,
      coordinateHeight: 3_375,
      regions: [{ centerX: 3_000, centerY: 0 }],
    }, { width: 1_200, height: 800 }, { width: 6_000, height: 4_000 });
    expect(region.left + region.width / 2).toBeCloseTo(0.5);
    expect(region.top + region.height / 2).toBeCloseTo(312.5 / 4_000);
  });

  it("maps through a centered square preview crop and hides cropped-out points", () => {
    const [region] = mapFocusRegions(focus(3_000, 1_000), { width: 800, height: 800 });
    expect(region.left + region.width / 2).toBeCloseTo(0.5);
    expect(region.top + region.height / 2).toBeCloseTo(0.25);
    expect(mapFocusRegions(focus(500, 2_000), { width: 800, height: 800 })).toEqual([]);
  });

  it("uses camera-provided frame dimensions when available", () => {
    const [region] = mapFocusRegions({
      coordinateWidth: 6_000,
      coordinateHeight: 4_000,
      regions: [{ centerX: 3_000, centerY: 2_000, width: 600, height: 400 }],
    }, { width: 1_200, height: 800 });
    expect(region.width).toBeCloseTo(0.1);
    expect(region.height).toBeCloseTo(0.1);
    expect(region.syntheticFrame).toBe(false);
  });

  it("maps the repository portrait HIF focus metadata onto macOS ImageIO dimensions", () => {
    const [region] = mapFocusRegions({
      coordinateWidth: 4_672,
      coordinateHeight: 7_008,
      regions: [{ centerX: 2_327, centerY: 1_489, width: 154, height: 153 }],
    }, { width: 2_730, height: 4_095 }, { width: 4_672, height: 7_008 });

    expect(region.left + region.width / 2).toBeCloseTo(2_327 / 4_672);
    expect(region.top + region.height / 2).toBeCloseTo(1_489 / 7_008);
    expect(region.syntheticFrame).toBe(false);
  });

});
