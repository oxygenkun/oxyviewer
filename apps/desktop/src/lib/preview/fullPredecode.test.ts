import { beforeEach, expect, it, vi } from "vitest";
import { create } from "zustand";
import type { AssetSummary, PreviewResult } from "@/types";

const mocks = vi.hoisted(() => ({ generated: vi.fn(), preload: vi.fn(), release: vi.fn() }));
vi.mock("@/lib/api", () => ({ generatedPreview: mocks.generated, previewUrl: () => undefined, renewMediaResource: async () => true }));
vi.mock("@/lib/cache/browserImageCache", () => ({ preloadBrowserImage: mocks.preload }));
vi.mock("@/lib/cache/mediaResourceLease", () => ({ retainMediaResource: () => mocks.release }));
const store = create<{ records: Record<string, { sourceRevision: string; status: string; result: PreviewResult }> }>(() => ({ records: {} }));
vi.mock("@/lib/projection/imageProjection", () => ({ imageProjectionKey: (path: string) => path, get useImageProjectionStore() { return store; } }));
const asset = { id: "raw", path: "/raw", kind: "raw" } as AssetSummary;
const interim = { url: "interim", satisfaction: "interim" } as PreviewResult;
const full = { url: "full", satisfaction: "satisfied", resource: { resourceId: "full-id" } } as PreviewResult;
beforeEach(() => {
  vi.clearAllMocks();
  store.setState({ records: { "/raw": { sourceRevision: "one", status: "loading", result: interim } } });
  mocks.generated.mockResolvedValue(interim);
  mocks.preload.mockResolvedValue(undefined);
});
it("holds the sequential slot through an interim RAW result and browser decode", async () => {
  const { predecodeFullImage } = await import("./fullPredecode");
  const loading = predecodeFullImage(asset, new AbortController().signal);
  await Promise.resolve();
  expect(mocks.preload).not.toHaveBeenCalled();
  store.setState({ records: { "/raw": { sourceRevision: "one", status: "ready", result: full } } });
  await loading;
  expect(mocks.generated).toHaveBeenCalledWith(asset, "full", expect.any(AbortSignal), "preload");
  expect(mocks.preload).toHaveBeenCalledWith("full", expect.any(AbortSignal), "full-id");
  expect(mocks.release).toHaveBeenCalledOnce();
});
it("releases the slot when an interim request is invalidated", async () => {
  const { predecodeFullImage } = await import("./fullPredecode");
  const loading = predecodeFullImage(asset, new AbortController().signal);
  const rejected = expect(loading).rejects.toThrow("invalidated");
  await Promise.resolve();
  store.setState({ records: {} });
  await rejected;
  expect(mocks.preload).not.toHaveBeenCalled();
});
it("cancels an interim waiter without accepting its eventual result", async () => {
  const { predecodeFullImage } = await import("./fullPredecode");
  const controller = new AbortController();
  const loading = predecodeFullImage(asset, controller.signal);
  const rejected = expect(loading).rejects.toBeDefined();
  await Promise.resolve();
  controller.abort();
  await rejected;
  store.setState({ records: { "/raw": { sourceRevision: "one", status: "ready", result: full } } });
  expect(mocks.preload).not.toHaveBeenCalled();
});
