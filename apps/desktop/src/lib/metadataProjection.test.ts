import { beforeEach, describe, expect, it } from "vitest";
import type { AssetSummary, MetadataProjection } from "../types";
import {
  acceptMetadataProjection,
  invalidateMetadataDirectory,
  projectAssetMetadata,
  useMetadataProjectionStore,
} from "./metadataProjection";

const asset: AssetSummary = {
  id: "one",
  path: "C:\\photos\\one.HIF",
  name: "one.HIF",
  extension: "HIF",
  kind: "heif",
  sizeBytes: 100,
  modifiedAtMs: 1,
  hasSidecar: false,
};

const projection = (projectionRevision: number, rating?: number): MetadataProjection => ({
  path: asset.path,
  sourceRevision: "source-1",
  projectionRevision,
  validAt: projectionRevision,
  status: "ready",
  rating,
  colorLabel: rating ? "Red" : undefined,
});

describe("metadata projection mirror", () => {
  beforeEach(() => useMetadataProjectionStore.setState({ records: {} }));

  it("rejects a late older revision", () => {
    acceptMetadataProjection(projection(2, 5));
    acceptMetadataProjection(projection(1, 1));

    const current = useMetadataProjectionStore.getState().records[asset.path];
    expect(current.rating).toBe(5);
  });

  it("treats an explicit missing value as authoritative", () => {
    acceptMetadataProjection(projection(1));
    const current = useMetadataProjectionStore.getState().records[asset.path];

    expect(projectAssetMetadata({ ...asset, rating: 4 }, current).rating).toBeUndefined();
  });

  it("drops display mirrors for an explicitly refreshed directory", () => {
    acceptMetadataProjection(projection(1, 4));
    invalidateMetadataDirectory("C:\\photos");

    expect(useMetadataProjectionStore.getState().records).toEqual({});
  });
});
