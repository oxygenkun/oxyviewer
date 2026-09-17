// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { FilmstripPreviewPreloader } from "./FilmstripPreviewPreloader";
import type { AssetSummary } from "@/types";

const preload = vi.hoisted(() => vi.fn(() => new Promise<void>(() => {})));
vi.mock("@/lib/preview/loupePreload", () => ({ preloadAssetLoupeBase: preload }));
vi.mock("@/lib/preview/previewQueue", () => ({ browserImageWorkerCount: 4 }));
vi.mock("@/lib/cache/browserImageCache", () => ({ protectBrowserImages: vi.fn() }));
vi.mock("@/lib/api", () => ({ previewUrl: () => undefined }));

it("keeps shared neighbors in flight and cancels only assets leaving the window", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const root = createRoot(document.createElement("div"));
  const asset = (id: string) => ({ id, path: id, modifiedAtMs: 1, name: id } as AssetSummary);
  const a = asset("a"), b = asset("b"), c = asset("c");
  const render = (assets: AssetSummary[]) => act(async () => root.render(
    <StrictMode><FilmstripPreviewPreloader assets={assets} /></StrictMode>,
  ));
  try {
    await render([a, b]);
    expect(preload).toHaveBeenCalledTimes(2);
    const aSignal = (preload.mock.calls[0] as unknown as [AssetSummary, number, AbortSignal])[2];
    const bSignal = (preload.mock.calls[1] as unknown as [AssetSummary, number, AbortSignal])[2];
    await render([a, b]);
    expect(preload).toHaveBeenCalledTimes(2);
    expect(aSignal.aborted).toBe(false);
    await render([b, c]);
    expect(aSignal.aborted).toBe(true);
    expect(bSignal.aborted).toBe(false);
    expect(preload).toHaveBeenCalledTimes(3);
    await act(async () => root.unmount());
    expect(bSignal.aborted).toBe(true);
  } finally {
    vi.unstubAllGlobals();
  }
});
