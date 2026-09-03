import { describe, expect, it } from "vitest";
import type { AssetDetails, AssetSummary, Page } from "../types";
import { patchAssetDetails, patchAssetPages, patchAssetSummaries } from "./metadataCache";

const asset: AssetSummary = {
  id: "one",
  path: "C:\\photos\\one.jpg",
  name: "one.jpg",
  extension: "jpg",
  kind: "jpeg",
  sizeBytes: 100,
  modifiedAtMs: 1,
  hasSidecar: false,
  rating: 2,
  colorLabel: "Red",
};

describe("metadata cache patches", () => {
  const paths = new Set([asset.path]);

  it("updates summaries in regular and paged caches", () => {
    expect(patchAssetSummaries([asset], paths, { rating: 5 })?.[0].rating).toBe(5);
    const data = { pages: [{ items: [asset], total: 1 } satisfies Page<AssetSummary>], pageParams: [0] };
    expect(patchAssetPages(data, paths, { colorLabel: "Blue" })?.pages[0].items[0].colorLabel)
      .toBe("Blue");
  });

  it("updates details and clears nullable metadata", () => {
    const details: AssetDetails = {
      asset,
      metadata: { rating: 2, colorLabel: "Red", keywords: [] },
      metadataCapability: {
        provider: "sidecar",
        readable: true,
        writable: true,
      },
      captureMetadata: {},
    };
    const patched = patchAssetDetails(details, paths, { rating: null, colorLabel: null });
    expect(patched?.asset.rating).toBeUndefined();
    expect(patched?.asset.colorLabel).toBeUndefined();
    expect(patched?.metadata.rating).toBeUndefined();
    expect(patched?.metadata.colorLabel).toBeUndefined();
  });
});
