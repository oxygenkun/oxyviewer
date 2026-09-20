import { beforeEach, describe, expect, it, vi } from "vitest";
import { clearBrowserImageResources, isBrowserImageReady, markBrowserImageReady } from "@/lib/cache/browserImageCache";
import type { ImageProjection } from "@/types";
import {
  acceptImageProjection,
  clearImageProjections,
  imageProjectionKey,
  invalidateImageAsset,
  invalidateImageDirectory,
  invalidateImageProjection,
  useImageProjectionStore,
} from "./imageProjection";

const releases = vi.hoisted(() => vi.fn());
vi.mock("@/lib/cache/mediaResourceLease", () => ({ releaseUnretainedMediaResource: releases }));

const projection = (stateRevision: number): ImageProjection => ({
  path: "C:\\photos\\one.HIF",
  sourceRevision: "source-1",
  stateRevision,
  validAt: stateRevision,
  status: "loading",
  level: "thumbnail",
});

describe("image projection mirror", () => {
  it("retires RAW full results without clearing thumbnail or decoded-image retention", () => {
    clearImageProjections();
    const raw = { ...projection(1), path: "C:\\photos\\a.arw", level: "full" as const };
    acceptImageProjection(raw);
    acceptImageProjection({ ...raw, level: "thumbnail" });
    acceptImageProjection({ ...raw, path: "C:\\photos\\b.arw" });
    markBrowserImageReady("retained-full.jpg", { width: 100, height: 60 });
    invalidateImageProjection(raw.path, "full");
    expect(useImageProjectionStore.getState().records[imageProjectionKey(raw.path, "full")]).toBeUndefined();
    expect(useImageProjectionStore.getState().records[imageProjectionKey(raw.path, "thumbnail")]).toBeDefined();
    expect(useImageProjectionStore.getState().records[imageProjectionKey("C:\\photos\\b.arw", "full")]).toBeDefined();
    expect(isBrowserImageReady("retained-full.jpg")).toBe(true);
    expect(acceptImageProjection({ ...raw, stateRevision: 2 })).toBe(false);
    expect(acceptImageProjection({ ...raw, stateRevision: 3, sourceRevision: "retry" })).toBe(true);
    clearImageProjections();
  });
  it("keeps decoded immutable resources through persistence updates", () => {
    clearBrowserImageResources();
    useImageProjectionStore.setState({ records: {} });
    const ready: ImageProjection = { ...projection(1), status: "ready",
      result: { path: "one.jpg", width: 1, height: 1, kind: "decoded", renderLevel: "thumbnail",
        resource: { resourceId: "same", url: "oxy-media://localhost/resource/same", mediaType: "image/jpeg" } } };
    acceptImageProjection(ready);
    const url = useImageProjectionStore.getState().records[imageProjectionKey(ready.path, ready.level)].result!.url;
    markBrowserImageReady(url, { width: 1, height: 1 });
    acceptImageProjection({ ...ready, stateRevision: 2 });
    expect(isBrowserImageReady(url)).toBe(true);
    acceptImageProjection({ ...ready, stateRevision: 3, sourceRevision: "changed" });
    expect(isBrowserImageReady(url)).toBe(false);
  });
  beforeEach(() => useImageProjectionStore.setState({ records: {} }));

  it("accepts only increasing Rust state revisions", () => {
    acceptImageProjection(projection(2));
    acceptImageProjection(projection(1));

    const key = imageProjectionKey("C:\\photos\\one.HIF", "thumbnail");
    expect(useImageProjectionStore.getState().records[key].stateRevision).toBe(2);
  });

  it("releases descriptors rejected by the revision fence, but accepts duplicate delivery", () => {
    const withResource = (revision: number, id: string): ImageProjection => ({
      ...projection(revision), status: "ready",
      result: { path: "one.jpg", width: 1, height: 1, kind: "decoded", renderLevel: "thumbnail",
        resource: { resourceId: id, url: `oxy-media://localhost/resource/${id}`, mediaType: "image/jpeg" } },
    });
    releases.mockClear();
    expect(acceptImageProjection(withResource(2, "current"))).toBe(true);
    expect(acceptImageProjection(withResource(1, "late"))).toBe(false);
    expect(releases).toHaveBeenCalledWith("late");
    expect(acceptImageProjection(withResource(2, "current"))).toBe(true);
    expect(releases).not.toHaveBeenCalledWith("current");
    expect(acceptImageProjection(withResource(3, "restored"))).toBe(true);
    expect(useImageProjectionStore.getState().records[imageProjectionKey(projection(1).path, "thumbnail")]
      .result?.resource?.resourceId).toBe("restored");
  });

  it("drops display mirrors for an explicitly refreshed directory", () => {
    acceptImageProjection(projection(1));
    invalidateImageDirectory("C:\\photos");

    expect(useImageProjectionStore.getState().records).toEqual({});
  });

  it("drops only the removed asset's mirrors", () => {
    acceptImageProjection(projection(1));
    acceptImageProjection({ ...projection(1), path: "C:\\photos\\two.HIF" });
    invalidateImageAsset(projection(1).path);

    expect(useImageProjectionStore.getState().records[imageProjectionKey(projection(1).path, "thumbnail")]).toBeUndefined();
    expect(useImageProjectionStore.getState().records[imageProjectionKey("C:\\photos\\two.HIF", "thumbnail")]).toBeDefined();
  });

  it("drops all artifact mirrors when the preview cache is cleared", () => {
    acceptImageProjection(projection(1));
    clearImageProjections();

    expect(useImageProjectionStore.getState().records).toEqual({});
  });
});
