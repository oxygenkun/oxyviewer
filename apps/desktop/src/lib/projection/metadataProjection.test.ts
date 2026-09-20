// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import type { AssetSummary, MetadataProjection } from "@/types";
import {
  acceptMetadataProjection,
  acceptMetadataProjections,
  applyMetadataProjectionPatch,
  invalidateMetadataAsset,
  invalidateMetadataDirectory,
  projectAssetMetadata,
  queueMetadataProjection,
  flushMetadataProjections,
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
  beforeEach(() => {
    flushMetadataProjections();
    useMetadataProjectionStore.setState({ records: {} });
  });

  it("retains asset identity for unchanged visible metadata", () => {
    expect(projectAssetMetadata(asset, projection(1))).toBe(asset);
    const first = projectAssetMetadata(asset, projection(2, 4));
    expect(projectAssetMetadata(asset, projection(3, 4))).toBe(first);
    const newSource = { ...asset, sizeBytes: 200 };
    expect(projectAssetMetadata(newSource, projection(3, 4))).not.toBe(first);
    expect(projectAssetMetadata(newSource, projection(3, 4)).sizeBytes).toBe(200);
  });

  it("batches notifications by newest revision and drops invalidated pending paths", () => {
    let updates = 0;
    const unsubscribe = useMetadataProjectionStore.subscribe(() => { updates += 1; });
    queueMetadataProjection(projection(3, 5));
    queueMetadataProjection(projection(2, 1));
    queueMetadataProjection({ ...projection(1, 2), path: "C:\\other\\two.jpg" });
    expect(updates).toBe(0);
    flushMetadataProjections();
    expect(updates).toBe(1);
    expect(useMetadataProjectionStore.getState().records[asset.path].rating).toBe(5);
    queueMetadataProjection(projection(4, 1));
    invalidateMetadataDirectory("C:\\photos");
    flushMetadataProjections();
    expect(useMetadataProjectionStore.getState().records[asset.path]).toBeUndefined();
    expect(useMetadataProjectionStore.getState().records["C:\\other\\two.jpg"]).toBeDefined();
    unsubscribe();
  });

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

  it("drops only a deleted asset's display mirror", () => {
    acceptMetadataProjection(projection(1, 4));
    acceptMetadataProjection({ ...projection(1, 2), path: "C:\\photos\\two.jpg" });
    invalidateMetadataAsset(asset.path);

    expect(useMetadataProjectionStore.getState().records[asset.path]).toBeUndefined();
    expect(useMetadataProjectionStore.getState().records["C:\\photos\\two.jpg"]).toBeDefined();
  });
});
