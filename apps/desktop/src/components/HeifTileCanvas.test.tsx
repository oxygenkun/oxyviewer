// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AssetSummary } from "../types";
import { HeifTileCanvas } from "./HeifTileCanvas";

const mocks = vi.hoisted(() => ({
  start: vi.fn(), cancel: vi.fn(), renew: vi.fn(), release: vi.fn(),
  listen: vi.fn(), mark: vi.fn(), perfActive: vi.fn(() => true),
}));
vi.mock("../lib/api", () => ({
  isTauri: () => true,
  startHeifFull: mocks.start,
  cancelHeifDecode: mocks.cancel,
  renewMediaResource: mocks.renew,
  releaseMediaResource: mocks.release,
  heifTileUrl: (url: string) => url,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("../lib/perfProbe", () => ({ perfMark: mocks.mark, isPerfActive: mocks.perfActive }));

const asset: AssetSummary = {
  id: "hif", path: "/photos/a.hif", name: "a.hif", extension: "hif",
  kind: "heif", sizeBytes: 100, modifiedAtMs: 1, hasSidecar: false,
};
const artifact = {
  delivery: "artifact", result: {
    path: "/cache/full.jpg", url: "oxy-media://localhost/resource/full",
    width: 6000, height: 4000, kind: "decoded", renderLevel: "full",
    resource: { resourceId: "full", url: "oxy-media://localhost/resource/full", mediaType: "image/jpeg" },
  },
};
let root: Root;
let container: HTMLDivElement;
const artifactDisplayed = vi.fn();
const imageSize = vi.fn();
const status = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  mocks.perfActive.mockReturnValue(true);
  mocks.renew.mockResolvedValue(true);
  mocks.release.mockResolvedValue(undefined);
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("__OXY_DEBUG__", false);
  // Never deliver animation frames: an occluded WKWebView can suspend them.
  vi.stubGlobal("requestAnimationFrame", vi.fn(() => 1));
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  mocks.listen.mockResolvedValue(vi.fn());
  mocks.start.mockResolvedValue({ delivery: "tiles", session: {
    id: "session", width: 6000, height: 4000, tileSize: 512,
    expectedTiles: 96, status: "decoding",
  } });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function render(selected = asset, displaySharpening = true) {
  await act(async () => root.render(
    <StrictMode><HeifTileCanvas asset={selected} displaySharpening={displaySharpening}
      onArtifactDisplayed={artifactDisplayed}
      onImageSize={imageSize} onStatus={status} /></StrictMode>,
  ));
}

describe("HEIF full presentation lifecycle", () => {
  it("displays the matching full artifact directly and requeries when sharpening changes", async () => {
    mocks.start.mockResolvedValue(artifact);
    const context = vi.spyOn(HTMLCanvasElement.prototype, "getContext");
    await render(asset, true);
    expect(mocks.start.mock.lastCall?.[2]).toBe(true);
    const pending = container.querySelector("img")!;
    expect(pending.src).toBe(artifact.result.url);
    await act(async () => pending.dispatchEvent(new Event("load")));
    expect(container.querySelector("img")?.style.visibility).not.toBe("hidden");
    expect(context).not.toHaveBeenCalled();
    const plain = { ...artifact, result: {
      ...artifact.result, url: "oxy-media://localhost/resource/plain",
      resource: { ...artifact.result.resource, resourceId: "plain", url: "oxy-media://localhost/resource/plain" },
    } };
    mocks.start.mockResolvedValue(plain);
    await render(asset, false);
    expect(mocks.start.mock.lastCall?.[2]).toBe(false);
    expect([...container.querySelectorAll("img")].map((image) => image.src)).not.toContain(artifact.result.url);
    expect(container.querySelector("img")?.src).toBe(plain.result.url);
    expect(context).not.toHaveBeenCalled();
  });

  it("does not draw a late bitmap onto the next selection and always closes it", async () => {
    const drawImage = vi.fn();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage } as unknown as CanvasRenderingContext2D);
    const fetchTile = vi.fn().mockResolvedValue({ ok: true, blob: async () => new Blob(["jpeg"]) });
    vi.stubGlobal("fetch", fetchTile);
    let complete!: (bitmap: ImageBitmap) => void;
    const bitmap = { close: vi.fn() } as unknown as ImageBitmap;
    vi.stubGlobal("createImageBitmap", vi.fn(() => new Promise<ImageBitmap>((resolve) => { complete = resolve; })));
    await render();
    const listener = mocks.listen.mock.calls.find(([name]) => name === "heif-tile-ready")![1];
    await act(async () => listener({ payload: {
      sessionId: "session", generation: mocks.start.mock.calls[0][1],
      url: "tile", payload: "jpeg", x: 0, y: 0, width: 512, height: 512,
    } }));
    expect(createImageBitmap).toHaveBeenCalledTimes(1);
    await render({ ...asset, id: "next", path: "/photos/b.hif", name: "b.hif" });
    await act(async () => complete(bitmap));
    expect(drawImage).not.toHaveBeenCalled();
    expect(bitmap.close).toHaveBeenCalledTimes(1);
    expect(fetchTile.mock.calls[0][1].signal.aborted).toBe(true);
  });

  it("ignores a disposed session's late startup failure", async () => {
    let reject!: (error: Error) => void;
    mocks.start.mockReturnValueOnce(new Promise((_, fail) => { reject = fail; }));
    await render();
    await render({ ...asset, id: "next", path: "/photos/b.hif", name: "b.hif" });
    status.mockClear();
    await act(async () => reject(new Error("old session failed")));
    expect(status).not.toHaveBeenCalled();
  });

  it("starts once in StrictMode without waiting for a paint frame", async () => {
    await render();
    expect(mocks.listen).toHaveBeenCalledTimes(2);
    expect(mocks.start).toHaveBeenCalledTimes(1);
    expect(imageSize).toHaveBeenCalledWith({ width: 6000, height: 4000 });
    const signal = mocks.start.mock.calls[0][3] as AbortSignal;
    expect(signal.aborted).toBe(false);
    await act(async () => root.render(null));
    expect(signal.aborted).toBe(true);
    expect(mocks.cancel).toHaveBeenCalledWith("session");
  });

  it("leases an artifact and records actual image loading before releasing it", async () => {
    mocks.start.mockResolvedValue(artifact);
    await render();
    expect(mocks.renew).toHaveBeenCalledWith("full");
    expect(artifactDisplayed).not.toHaveBeenCalled();
    expect(mocks.mark).not.toHaveBeenCalledWith("image:loaded", expect.anything());
    await act(async () => container.querySelector("img")!.dispatchEvent(new Event("load")));
    expect(mocks.mark).toHaveBeenCalledWith("image:loaded", expect.objectContaining({ stage: "full" }));
    expect(artifactDisplayed).toHaveBeenCalledTimes(1);
    await act(async () => root.render(null));
    expect(mocks.release).toHaveBeenCalledWith("full");
  });

  it("recovers expired descriptors and holds the displayed artifact until its replacement loads", async () => {
    vi.useFakeTimers();
    try {
      mocks.start.mockResolvedValue(artifact);
      await render();
      await act(async () => container.querySelector("img")!.dispatchEvent(new Event("load")));
      const fresh = { ...artifact, result: { ...artifact.result, url: "oxy-media://localhost/resource/fresh",
        resource: { ...artifact.result.resource, resourceId: "fresh", url: "oxy-media://localhost/resource/fresh" } } };
      mocks.start.mockResolvedValue(fresh);
      mocks.renew.mockImplementation((id: string) => Promise.resolve(id !== "full"));
      await act(async () => vi.advanceTimersByTimeAsync(10_000));
      expect(container.querySelectorAll("img")).toHaveLength(2);
      expect(mocks.release).not.toHaveBeenCalledWith("full");
      await act(async () => container.querySelector<HTMLImageElement>("img[src$='/fresh']")!.dispatchEvent(new Event("load")));
      expect(container.querySelectorAll("img")).toHaveLength(1);
      expect(mocks.release).toHaveBeenCalledWith("full");
      mocks.start.mockClear();
      await act(async () => vi.advanceTimersByTimeAsync(10_000));
      expect(mocks.start).not.toHaveBeenCalled();
    } finally { vi.useRealTimers(); }
  });

  it("keeps the canvas available when an expired artifact recovers through tiles", async () => {
    vi.useFakeTimers();
    try {
      vi.stubGlobal("requestAnimationFrame", vi.fn(() => 1));
      mocks.perfActive.mockReturnValue(false);
      mocks.start.mockResolvedValue(artifact);
      await render();
      await act(async () => container.querySelector("img")!.dispatchEvent(new Event("load")));
      expect(container.querySelector("canvas")).not.toBeNull();
      mocks.start.mockResolvedValue({ delivery: "tiles", session: {
        id: "replacement", width: 512, height: 512, tileSize: 512,
        expectedTiles: 1, status: "decoding",
      } });
      mocks.renew.mockResolvedValue(false);
      await act(async () => vi.advanceTimersByTimeAsync(10_000));
      expect(container.querySelector("img")).not.toBeNull();
      const drawImage = vi.fn();
      vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage } as unknown as CanvasRenderingContext2D);
      vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true, blob: async () => new Blob(["jpeg"]) }));
      vi.stubGlobal("createImageBitmap", vi.fn().mockResolvedValue({ close: vi.fn() }));
      const generation = mocks.start.mock.lastCall![1];
      const tileListener = mocks.listen.mock.calls.filter(([name]) => name === "heif-tile-ready").at(-1)![1];
      const statusListener = mocks.listen.mock.calls.filter(([name]) => name === "heif-decode-status").at(-1)![1];
      await act(async () => tileListener({ payload: {
        sessionId: "replacement", generation, url: "tile", payload: "jpeg",
        x: 0, y: 0, width: 512, height: 512,
      } }));
      await act(async () => statusListener({ payload: { sessionId: "replacement", generation, status: "complete" } }));
      const paint = vi.mocked(requestAnimationFrame).mock.lastCall![0];
      await act(async () => paint(0));
      expect(drawImage).toHaveBeenCalledTimes(1);
      expect(container.querySelector("img")).toBeNull();
      expect(mocks.release).toHaveBeenCalledWith("full");
    } finally { vi.useRealTimers(); }
  });

  it("releases an artifact returned after unmount", async () => {
    let complete!: (value: typeof artifact) => void;
    mocks.start.mockReturnValue(new Promise((resolve) => { complete = resolve; }));
    await render();
    await act(async () => root.render(null));
    await act(async () => complete(artifact));
    expect(mocks.release).toHaveBeenCalledWith("full");
    expect(imageSize).not.toHaveBeenCalled();
  });

  it("cleans up a listener installed after disposal without starting a session", async () => {
    let complete!: (value: () => void) => void;
    const stop = vi.fn();
    mocks.listen.mockReturnValueOnce(new Promise((resolve) => { complete = resolve; }));
    await render();
    await act(async () => root.render(null));
    await act(async () => complete(stop));
    expect(stop).toHaveBeenCalledTimes(1);
    expect(mocks.start).not.toHaveBeenCalled();
  });
});
