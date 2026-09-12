import { describe, expect, it, vi } from "vitest";
import type { AssetTagAssignmentsByPath } from "../types";
import { createAssetTagBatcher } from "./assetTagBatch";

describe("asset tag query batching", () => {
  it("combines duplicate paths and preserves empty results", async () => {
    const read = vi.fn(async (paths: string[]) => paths.map((path) => ({ path, assignments: [] })));
    const query = createAssetTagBatcher(read);
    expect(await Promise.all([query("a"), query("b"), query("a")])).toEqual([[], [], []]);
    expect(read.mock.calls).toEqual([[["a", "b"]]]);
  });

  it("rejects cancelled old queries while a refreshed query reads a new batch", async () => {
    const resolvers: ((rows: AssetTagAssignmentsByPath[]) => void)[] = [];
    const read = vi.fn(() => new Promise<AssetTagAssignmentsByPath[]>((resolve) => resolvers.push(resolve)));
    const query = createAssetTagBatcher(read);
    const controller = new AbortController();
    const old = query("a", controller.signal).catch((error: DOMException) => error.name);
    await Promise.resolve();
    controller.abort();
    const current = query("a");
    await Promise.resolve();
    resolvers[1]([{ path: "a", assignments: [] }]);
    expect(await current).toEqual([]);
    resolvers[0]([]);
    expect(await old).toBe("AbortError");
    expect(read).toHaveBeenCalledTimes(2);
  });
});
