import { describe, expect, it } from "vitest";
import {
  activeAssetIndex,
  activeAssetOrdinal,
  focusRestoreAction,
  gridRowCount,
  gridRowForAsset,
  replacementAssetIdAfterRemoval,
  replacementAssetIdAfterRemovals,
  virtualAssetCount,
} from "./assetViewPosition";

describe("asset view position", () => {
  const assets = [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }, { id: "e" }];

  it("finds the active asset without falling back to the first item", () => {
    expect(activeAssetIndex(assets, "d")).toBe(3);
    expect(activeAssetIndex(assets, "missing")).toBeUndefined();
    expect(activeAssetIndex(assets, undefined)).toBeUndefined();
  });

  it("reports the active asset's 1-based position and nothing when unselected", () => {
    expect(activeAssetOrdinal(assets, "a")).toBe(1);
    expect(activeAssetOrdinal(assets, "d")).toBe(4);
    expect(activeAssetOrdinal(assets, "missing")).toBeUndefined();
    expect(activeAssetOrdinal(assets, undefined)).toBeUndefined();
  });

  it("keeps paging while restoring a filtered filmstrip focus", () => {
    expect(focusRestoreAction(assets, "e", true, false)).toBe("none");
    expect(focusRestoreAction(assets.slice(0, 2), "e", true, false)).toBe("fetch");
    expect(focusRestoreAction(assets.slice(0, 2), "e", true, true)).toBe("wait");
    expect(focusRestoreAction(assets.slice(0, 2), "e", false, false)).toBe("fallback");
    expect(focusRestoreAction(assets, undefined, true, false)).toBe("none");
  });

  it("selects the asset that moves into the removed asset's position", () => {
    expect(replacementAssetIdAfterRemoval(assets, "c")).toBe("d");
    expect(replacementAssetIdAfterRemoval(assets, "e")).toBe("d");
    expect(replacementAssetIdAfterRemoval([{ id: "only" }], "only")).toBeUndefined();
    expect(replacementAssetIdAfterRemoval(assets, "missing")).toBeUndefined();
    expect(replacementAssetIdAfterRemovals(assets, ["b", "c", "d"], "c")).toBe("e");
    expect(replacementAssetIdAfterRemovals(assets, ["c", "d", "e"], "d")).toBe("b");
    expect(replacementAssetIdAfterRemovals(assets, assets.map((asset) => asset.id), "c")).toBeUndefined();
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
