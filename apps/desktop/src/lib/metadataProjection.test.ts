import { beforeEach, describe, expect, it } from "vitest";
import type { AssetSummary, MetadataProjection } from "../types";
import {
  acceptMetadataProjection,
  acceptMetadataProjections,
  applyMetadataProjectionPatch,
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

const projection = (stateRevision: number, rating?: number): MetadataProjection => ({
  path: asset.path,
  sourceRevision: "source-1",
  stateRevision,
  validAt: stateRevision,
  status: "ready",
  rating,
  colorLabel: rating ? "Red" : undefined,
  pickLabel: rating ? "accepted" : undefined,
});

describe("metadata projection mirror", () => {
  beforeEach(() => useMetadataProjectionStore.setState({ records: {} }));

  it("rejects a late older revision", () => {
    acceptMetadataProjection(projection(2, 5));
    acceptMetadataProjection(projection(1, 1));

    const current = useMetadataProjectionStore.getState().records[asset.path];
    expect(current.rating).toBe(5);
    expect(current.pickLabel).toBe("accepted");
  });

  it("accepts a page of projections in one store update", () => {
    let updates = 0;
    const unsubscribe = useMetadataProjectionStore.subscribe(() => { updates += 1; });
    acceptMetadataProjections([
      projection(2, 5),
      { ...projection(1, 1), path: "C:\\photos\\two.jpg" },
      projection(1, 1),
    ]);
    unsubscribe();

    expect(updates).toBe(1);
    expect(useMetadataProjectionStore.getState().records[asset.path].rating).toBe(5);
    expect(useMetadataProjectionStore.getState().records["C:\\photos\\two.jpg"].rating).toBe(1);
  });

  it("treats an explicit missing value as authoritative", () => {
    acceptMetadataProjection(projection(1));
    const current = useMetadataProjectionStore.getState().records[asset.path];

    expect(projectAssetMetadata({ ...asset, rating: 4 }, current).rating).toBeUndefined();
    expect(projectAssetMetadata({ ...asset, pickLabel: "rejected" }, current).pickLabel).toBeUndefined();
  });

  it("applies successful assignment and clear patches to the display mirror", () => {
    acceptMetadataProjection(projection(1, 4));

    applyMetadataProjectionPatch([asset.path], { colorLabel: "Purple", pickLabel: "pending" });
    let current = useMetadataProjectionStore.getState().records[asset.path];
    expect(current.colorLabel).toBe("Purple");
    expect(current.pickLabel).toBe("pending");
    expect(current.rating).toBe(4);

    applyMetadataProjectionPatch([asset.path], { colorLabel: null, pickLabel: null });
    current = useMetadataProjectionStore.getState().records[asset.path];
    expect(current.colorLabel).toBeUndefined();
    expect(current.pickLabel).toBeUndefined();
    expect(current.rating).toBe(4);
  });

  it("drops display mirrors for an explicitly refreshed directory", () => {
    acceptMetadataProjection(projection(1, 4));
    invalidateMetadataDirectory("C:\\photos");

    expect(useMetadataProjectionStore.getState().records).toEqual({});
  });
});
