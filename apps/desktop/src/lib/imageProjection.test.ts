import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ImageProjection } from "../types";
import {
  acceptImageProjection,
  clearImageProjections,
  imageProjectionKey,
  invalidateImageDirectory,
  useImageProjectionStore,
} from "./imageProjection";

const releases = vi.hoisted(() => vi.fn());
vi.mock("./mediaResourceLease", () => ({ releaseUnretainedMediaResource: releases }));

const projection = (projectionRevision: number): ImageProjection => ({
  path: "C:\\photos\\one.HIF",
  sourceRevision: "source-1",
  projectionRevision,
  validAt: projectionRevision,
  status: "loading",
  level: "thumbnail",
});

describe("image projection mirror", () => {
  beforeEach(() => useImageProjectionStore.setState({ records: {} }));

  it("accepts only increasing Rust projection revisions", () => {
    acceptImageProjection(projection(2));
    acceptImageProjection(projection(1));

    const key = imageProjectionKey("C:\\photos\\one.HIF", "thumbnail");
    expect(useImageProjectionStore.getState().records[key].projectionRevision).toBe(2);
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

  it("drops all artifact mirrors when the preview cache is cleared", () => {
    acceptImageProjection(projection(1));
    clearImageProjections();

    expect(useImageProjectionStore.getState().records).toEqual({});
  });
});
