import { describe, expect, it } from "vitest";
import {
  MARKING_ACTIONS,
  colorLabelForAction,
  markingPatchForAction,
  ratingForAction,
} from "./assetMarking";

describe("ratingForAction", () => {
  it("maps rating actions to their value", () => {
    expect(ratingForAction("marking.rating1")).toBe(1);
    expect(ratingForAction("marking.rating5")).toBe(5);
  });

  it("maps the clear action to null", () => {
    expect(ratingForAction("marking.clearRating")).toBeNull();
  });

  it("returns undefined for non-rating actions", () => {
    expect(ratingForAction("marking.colorRed")).toBeUndefined();
    expect(ratingForAction("grid.moveLeft")).toBeUndefined();
  });
});

describe("colorLabelForAction", () => {
  it("maps color actions to labels", () => {
    expect(colorLabelForAction("marking.colorRed")).toBe("Red");
    expect(colorLabelForAction("marking.colorBlue")).toBe("Blue");
  });

  it("returns undefined for non-color actions", () => {
    expect(colorLabelForAction("marking.rating1")).toBeUndefined();
  });
});

describe("markingPatchForAction", () => {
  it("builds rating patches", () => {
    expect(markingPatchForAction("marking.rating3", undefined)).toEqual({ rating: 3 });
    expect(markingPatchForAction("marking.clearRating", undefined)).toEqual({ rating: null });
  });

  it("sets a color when it differs from the current one", () => {
    expect(markingPatchForAction("marking.colorRed", "Blue")).toEqual({ colorLabel: "Red" });
    expect(markingPatchForAction("marking.colorRed", undefined)).toEqual({ colorLabel: "Red" });
  });

  it("clears the color when it is already active", () => {
    expect(markingPatchForAction("marking.colorRed", "red")).toEqual({ colorLabel: null });
  });

  it("returns undefined for non-marking actions", () => {
    expect(markingPatchForAction("loupe.nextAsset", undefined)).toBeUndefined();
  });
});

describe("MARKING_ACTIONS", () => {
  it("covers every marking action exactly once", () => {
    expect(MARKING_ACTIONS).toHaveLength(10);
    expect(new Set(MARKING_ACTIONS).size).toBe(10);
  });
});
