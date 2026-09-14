import { describe, expect, it } from "vitest";
import type { AssetQuery, AssetSummary, PickLabel } from "../types";
import { filterAndSortAssets } from "./assetFiltering";

const asset = (
  id: string,
  rating?: number,
  colorLabel?: string,
  pickLabel?: PickLabel,
): AssetSummary => ({
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
  pickLabel,
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

  it("keeps only the flagged assets and treats the flag group as OR", () => {
    const batch = [
      asset("accepted", undefined, undefined, "accepted"),
      asset("pending", undefined, undefined, "pending"),
      asset("rejected", undefined, undefined, "rejected"),
      asset("unflagged"),
      asset("unresolved"),
    ];
    expect(filterAndSortAssets(batch, { ...query, minimumRating: undefined, colorLabels: undefined, pickLabels: ["accepted"] })
      .map(({ id }) => id)).toEqual(["accepted"]);

    // Flags are case-insensitive and OR-combined, like color labels.
    expect(filterAndSortAssets(batch, {
      ...query,
      minimumRating: undefined,
      colorLabels: undefined,
      pickLabels: ["Accepted", "rejected"],
    }).map(({ id }) => id)).toEqual(["accepted", "rejected"]);

    // Clearing the flag group re-admits every asset.
    expect(filterAndSortAssets(batch, { ...query, minimumRating: undefined, colorLabels: undefined, pickLabels: [] }))
      .toHaveLength(5);
  });

  it("combines the flag group with the other metadata filters using AND", () => {
    const batch = [
      asset("match", 5, "Blue", "accepted"),
      asset("wrong-flag", 5, "Blue", "rejected"),
      asset("low-rating", 3, "Blue", "accepted"),
    ];
    expect(filterAndSortAssets(batch, { ...query, pickLabels: ["accepted"] }).map(({ id }) => id))
      .toEqual(["match"]);
  });
});
