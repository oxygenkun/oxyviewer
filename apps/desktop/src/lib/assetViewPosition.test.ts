import { describe, expect, it } from "vitest";
import { activeAssetIndex, gridRowForAsset } from "./assetViewPosition";

describe("asset view position", () => {
  const assets = [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }, { id: "e" }];

  it("finds the active asset without falling back to the first item", () => {
    expect(activeAssetIndex(assets, "d")).toBe(3);
    expect(activeAssetIndex(assets, "missing")).toBeUndefined();
    expect(activeAssetIndex(assets, undefined)).toBeUndefined();
  });

  it("maps the active asset to its virtual grid row", () => {
    expect(gridRowForAsset(3, 2)).toBe(1);
    expect(gridRowForAsset(4, 2)).toBe(2);
    expect(gridRowForAsset(4, 3)).toBe(1);
  });
});
