import { describe, expect, it } from "vitest";
import { expectedHeifTiles, HeifTileProgressTracker } from "./heifTileProgress";

describe("HEIF tile progress", () => {
  it("computes edge tiles for non-divisible image dimensions", () => {
    expect(expectedHeifTiles(1_200, 800, 512)).toBe(6);
  });

  it("tracks first draw, failures, completion, and ignores duplicates", () => {
    const progress = new HeifTileProgressTracker(2);
    expect(progress.receive({ x: 0, y: 0 }).first).toBe(true);
    expect(progress.receive({ x: 0, y: 0 }).accepted).toBe(false);
    expect(progress.settle({ x: 0, y: 0 }, true).firstDrawn).toBe(true);

    progress.receive({ x: 512, y: 0 });
    const final = progress.settle({ x: 512, y: 0 }, false).snapshot;
    expect(final).toEqual({
      expected: 2,
      received: 2,
      settled: 2,
      drawn: 1,
      failed: 1,
      allSettled: true,
    });
  });
});
