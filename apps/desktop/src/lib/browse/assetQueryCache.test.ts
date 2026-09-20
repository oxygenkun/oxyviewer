import { describe, expect, it } from "vitest";
import type { AssetSummary, Page } from "@/types";
import { removeAssetFromInfiniteData } from "./assetQueryCache";

const asset = (id: string, path = `/photos/${id}.jpg`): AssetSummary => ({
  id,
  path,
  name: `${id}.jpg`,
  extension: "jpg",
  kind: "jpeg",
  sizeBytes: 100,
  modifiedAtMs: 1,
  hasSidecar: false,
});

describe("removeAssetFromInfiniteData", () => {
  it("removes one asset across cached pages and updates the shared total", () => {
    const first: Page<AssetSummary> = {
      items: [asset("a"), asset("b")],
      nextCursor: 2,
      total: 3,
      snapshotRevision: 7,
    };
    const second: Page<AssetSummary> = {
      items: [asset("c")],
      total: 3,
      snapshotRevision: 7,
    };
    const result = removeAssetFromInfiniteData(
      { pages: [first, second], pageParams: [{ offset: 0 }, { offset: 2, snapshotRevision: 7 }] },
      "/photos/b.jpg",
    );

    expect(result?.pages.map((page) => page.items.map((item) => item.id))).toEqual([["a"], ["c"]]);
    expect(result?.pages.map((page) => page.total)).toEqual([2, 2]);
    expect(result?.pageParams).toEqual([{ offset: 0 }, { offset: 2, snapshotRevision: 7 }]);
  });

  it("preserves the data identity when the path is absent", () => {
    const data = {
      pages: [{ items: [asset("a")], total: 1 }],
      pageParams: [{ offset: 0 }],
    };

    expect(removeAssetFromInfiniteData(data, "/photos/missing.jpg")).toBe(data);
  });
});
