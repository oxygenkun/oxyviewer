import { beforeEach, describe, expect, it } from "vitest";
import type { ImageProjection } from "../types";
import {
  acceptImageProjection,
  clearImageProjections,
  imageProjectionKey,
  invalidateImageDirectory,
  useImageProjectionStore,
} from "./imageProjection";

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
