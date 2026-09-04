import { describe, expect, it } from "vitest";
import {
  activeAssetIndex,
  gridRowCount,
  gridRowForAsset,
  replacementAssetIdAfterRemoval,
  virtualAssetCount,
} from "./assetViewPosition";

describe("asset view position", () => {
  const assets = [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }, { id: "e" }];

  it("finds the active asset without falling back to the first item", () => {
    expect(activeAssetIndex(assets, "d")).toBe(3);
    expect(activeAssetIndex(assets, "missing")).toBeUndefined();
    expect(activeAssetIndex(assets, undefined)).toBeUndefined();
  });

  it("selects the asset that moves into the removed asset's position", () => {
    expect(replacementAssetIdAfterRemoval(assets, "c")).toBe("d");
    expect(replacementAssetIdAfterRemoval(assets, "e")).toBe("d");
    expect(replacementAssetIdAfterRemoval([{ id: "only" }], "only")).toBeUndefined();
    expect(replacementAssetIdAfterRemoval(assets, "missing")).toBeUndefined();
  });

  it("maps the active asset to its virtual grid row", () => {
    expect(gridRowForAsset(3, 2)).toBe(1);
    expect(gridRowForAsset(4, 2)).toBe(2);
    expect(gridRowForAsset(4, 3)).toBe(1);
  });

  it("sizes virtual views from the folder total before every page is loaded", () => {
    expect(virtualAssetCount(250, 1_001)).toBe(1_001);
    expect(gridRowCount(1_001, 5)).toBe(201);
  });

  it("does not truncate already loaded assets when totals become stale", () => {
    expect(virtualAssetCount(251, 250)).toBe(251);
  });
});
