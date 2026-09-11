// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { BackgroundPreviewPreloader } from "./BackgroundPreviewPreloader";
import type { AssetSummary } from "../types";

const mock = vi.hoisted(() => ({ ready: new Set<string>(), generation: 0, preload: vi.fn() }));
vi.mock("../lib/api", () => ({ preloadAssetThumbnail: mock.preload }));
vi.mock("../lib/folderThumbnailCache", () => ({
  getFolderThumbnail: (asset: AssetSummary) => mock.ready.has(asset.id) ? {} : undefined,
  folderThumbnailKey: (asset: AssetSummary) => asset.id,
  useFolderThumbnailGeneration: () => mock.generation,
}));
let root: ReturnType<typeof createRoot>;
const assets = ["a", "b", "c"].map((id) => ({ id, name: id } as AssetSummary));
const jobs: Array<{ id: string; signal: AbortSignal; finish: () => void; fail: () => void }> = [];
const render = async (files: AssetSummary[]) => {
  await act(async () => root.render(<BackgroundPreviewPreloader assets={files} />));
};
const advance = async (ms = 200) => { await act(async () => vi.advanceTimersByTimeAsync(ms)); };
beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  mock.ready.clear();
  mock.generation = 0;
  jobs.length = 0;
  mock.preload.mockReset().mockImplementation((asset: AssetSummary, signal: AbortSignal) => new Promise<void>((resolve, reject) => {
    signal.addEventListener("abort", () => reject(signal.reason), { once: true });
    jobs.push({ id: asset.id, signal,
      finish: () => { mock.ready.add(asset.id); resolve(); },
      fail: () => reject(new Error("Unreadable file")),
    });
  }));
  root = createRoot(document.createElement("div"));
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it("warms ordinary off-screen files after the first paint and continues through appended pages", async () => {
  await render(assets.slice(0, 2));
  expect(mock.preload).not.toHaveBeenCalled();
  await advance();
  expect(jobs.map((job) => job.id)).toEqual(["a"]);
  await render(assets);
  expect(jobs[0].signal.aborted).toBe(false);
  await act(async () => jobs[0].finish());
  await advance();
  expect(jobs.map((job) => job.id)).toEqual(["a", "b"]);
  await act(async () => jobs[1].finish());
  await advance();
  expect(jobs.map((job) => job.id)).toEqual(["a", "b", "c"]);
  await act(async () => jobs[2].finish());
  await advance();
  expect(mock.preload).toHaveBeenCalledTimes(3);
});

it("skips retained thumbnails and aborts work leaving the candidate directory", async () => {
  mock.ready.add("a");
  await render(assets);
  await advance();
  expect(jobs[0].id).toBe("b");
  await render([assets[2]]);
  await advance();
  expect(jobs[0].signal.aborted).toBe(true);
  expect(jobs[1].id).toBe("c");
});

it("restarts warming after explicit cache invalidation", async () => {
  mock.ready.add("a");
  await render([assets[0]]);
  await advance();
  expect(jobs).toHaveLength(0);
  mock.ready.clear();
  mock.generation += 1;
  await render([assets[0]]);
  await advance();
  expect(jobs[0].id).toBe("a");
});
