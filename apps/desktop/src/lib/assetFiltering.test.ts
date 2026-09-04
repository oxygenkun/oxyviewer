import { describe, expect, it } from "vitest";
import type { AssetQuery, AssetSummary } from "../types";
import { filterAndSortAssets } from "./assetFiltering";

const asset = (id: string, rating?: number, colorLabel?: string): AssetSummary => ({
  id,
  path: `/photos/${id}.jpg`,
  name: `${id}.jpg`,
  extension: "JPG",
  kind: "jpeg",
  sizeBytes: 100,
  modifiedAtMs: 1,
  hasSidecar: false,
  rating,
  colorLabel,
});

describe("progressive asset filtering", () => {
  const query: AssetQuery = {
    minimumRating: 4,
    colorLabels: ["Blue", "Red"],
    sort: "name",
    direction: "ascending",
    pageSize: 250,
  };

  it("shows matches from each enriched batch without admitting unresolved candidates", () => {
    const firstBatch = [asset("b", 5, "Blue"), asset("unresolved"), asset("red", 5, "Red")];
    expect(filterAndSortAssets(firstBatch, query).map(({ id }) => id)).toEqual(["b", "red"]);

    const nextBatch = [...firstBatch, asset("a", 4, "blue"), asset("low", 3, "Blue")];
    expect(filterAndSortAssets(nextBatch, query).map(({ id }) => id)).toEqual(["a", "b", "red"]);
  });
});
