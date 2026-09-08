// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AssetSummary } from "../types";
import { HeifTileCanvas } from "./HeifTileCanvas";

const mocks = vi.hoisted(() => ({
  start: vi.fn(), cancel: vi.fn(), renew: vi.fn(), release: vi.fn(),
  listen: vi.fn(), mark: vi.fn(),
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
vi.mock("../lib/perfProbe", () => ({ perfMark: mocks.mark, isPerfActive: () => true }));

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
const imageSize = vi.fn();
const status = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
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

async function render(selected = asset) {
  await act(async () => root.render(
    <StrictMode><HeifTileCanvas asset={selected} displaySharpening
      onImageSize={imageSize} onStatus={status} /></StrictMode>,
  ));
}

describe("HEIF full presentation lifecycle", () => {
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
    await act(async () => root.render(null));
    expect(mocks.cancel).toHaveBeenCalledWith("session");
  });

  it("leases an artifact and records actual image loading before releasing it", async () => {
    mocks.start.mockResolvedValue(artifact);
    await render();
    expect(mocks.renew).toHaveBeenCalledWith("full");
    expect(mocks.mark).not.toHaveBeenCalledWith("image:loaded", expect.anything());
    await act(async () => container.querySelector("img")!.dispatchEvent(new Event("load")));
    expect(mocks.mark).toHaveBeenCalledWith("image:loaded", expect.objectContaining({ stage: "full" }));
    await act(async () => root.render(null));
    expect(mocks.release).toHaveBeenCalledWith("full");
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
