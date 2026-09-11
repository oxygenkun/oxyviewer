// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Thumbnail } from "../components/Thumbnail";
import type { AssetSummary } from "../types";
import {
  captureFolderThumbnail, clearFolderThumbnails, discardFolderThumbnail,
  getFolderThumbnail, getFolderThumbnailStats, preloadFolderThumbnail,
} from "./folderThumbnailCache";
import { setBrowserImageResourceScope } from "./browserImageCache";
import { useImageProjectionStore } from "./imageProjection";

const native = vi.hoisted(() => ({ request: vi.fn(), renew: vi.fn(), release: vi.fn() }));
vi.mock("./api", () => ({
  isTauri: () => true, previewUrl: () => undefined,
  generatedPreview: native.request, renewMediaResource: native.renew,
  releaseMediaResource: native.release,
}));
const createUrl = vi.fn();
const revokeUrl = vi.fn();
let serial = 0;

class DecodedImage {
  crossOrigin = "";
  naturalWidth = 512;
  naturalHeight = 384;
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  value = "";
  set src(value: string) {
    this.value = value;
    if (value) queueMicrotask(() => this.onload?.());
  }
  get src() { return this.value; }
  decode() { return Promise.resolve(); }
}

const asset = (id: string, modifiedAtMs = 1) => ({
  id, path: `/photos/${id}.hif`, name: `${id}.hif`, extension: "hif",
  kind: "heif", sizeBytes: 1000, modifiedAtMs, hasSidecar: false,
} as AssetSummary);
function input(width = 4000, height = 3000) {
  const image = document.createElement("img");
  Object.defineProperties(image, {
    naturalWidth: { value: width }, naturalHeight: { value: height },
  });
  return image;
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("__OXY_DEBUG__", false);
  vi.stubGlobal("Image", DecodedImage);
  vi.stubGlobal("URL", { createObjectURL: createUrl, revokeObjectURL: revokeUrl });
  createUrl.mockImplementation(() => `blob:thumbnail-${serial++}`);
  revokeUrl.mockClear();
  native.request.mockClear();
  native.renew.mockReset().mockResolvedValue(false);
  native.release.mockReset().mockResolvedValue(undefined);
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows");
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage: vi.fn() } as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLCanvasElement.prototype, "toBlob").mockImplementation((callback) => callback(new Blob(["thumbnail"])));
  clearFolderThumbnails();
});
afterEach(() => {
  clearFolderThumbnails();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("retains the whole folder beyond both generic image-cache limits without native leases", async () => {
  const source = input();
  for (let index = 0; index < 1100; index++) await captureFolderThumbnail(asset(`${index}`), source);
  expect(getFolderThumbnailStats().count).toBe(1100);
  expect(getFolderThumbnailStats().decodedBytes).toBeGreaterThan(512 * 1024 * 1024);
  expect(getFolderThumbnail(asset("0"))?.image).toBeDefined();
  expect(getFolderThumbnail(asset("1099"))?.image).toBeDefined();
  expect(native.renew).not.toHaveBeenCalled();
  expect(native.release).not.toHaveBeenCalled();
});

it("retains small encoded thumbnails with one Blob URL and no canvas re-encoding", async () => {
  const blob = new Blob(["jpeg"], { type: "image/jpeg" });
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true, blob: async () => blob }));
  createUrl.mockClear();
  await preloadFolderThumbnail(asset("encoded"), "https://media/thumbnail");
  const retained = getFolderThumbnail(asset("encoded"));
  expect(retained?.image.src).toBe(retained?.url);
  expect(retained?.width).toBe(512);
  expect(createUrl).toHaveBeenCalledExactlyOnceWith(blob);
  expect(HTMLCanvasElement.prototype.toBlob).not.toHaveBeenCalled();
  expect(revokeUrl).not.toHaveBeenCalled();
  clearFolderThumbnails();
  expect(revokeUrl).toHaveBeenCalledWith(retained?.url);
});

it("fences an encoded response that arrives after the folder changes", async () => {
  let finish!: (value: Blob) => void;
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true, blob: () => new Promise<Blob>((resolve) => { finish = resolve; }) }));
  const warming = preloadFolderThumbnail(asset("late-encoded"), "https://media/thumbnail");
  await Promise.resolve();
  clearFolderThumbnails();
  finish(new Blob(["jpeg"]));
  await warming;
  expect(getFolderThumbnail(asset("late-encoded"))).toBeUndefined();
});

it("shrinks originals and scales the content rectangle while preserving display geometry", async () => {
  const retained = await captureFolderThumbnail(asset("geometry"), input(), {
    displaySize: { width: 6000, height: 4000 },
    contentRect: { x: 1000, y: 750, width: 2000, height: 1500 },
  });
  expect(retained).toMatchObject({ width: 512, height: 384, geometry: {
    displaySize: { width: 6000, height: 4000 },
    contentRect: { x: 128, y: 96, width: 256, height: 192 },
  } });
  expect(retained?.url).toMatch(/^blob:/);
});

it("reuses the same decoded thumbnail across virtual mounts without re-requesting native media", async () => {
  const file = asset("cached");
  const retained = await captureFolderThumbnail(file, input(160, 120));
  const client = new QueryClient();
  const container = document.createElement("div");
  const mounted = createRoot(container);
  try {
    await act(async () => mounted.render(<QueryClientProvider client={client}><Thumbnail asset={file} /></QueryClientProvider>));
    expect(container.querySelector("img")?.getAttribute("src")).toBe(retained?.url);
    expect(container.querySelector(".thumbnail__fallback")).toBeNull();
    await act(async () => mounted.render(null));
    await act(async () => mounted.render(<QueryClientProvider client={client}><Thumbnail asset={file} /></QueryClientProvider>));
    expect(container.querySelector("img")?.getAttribute("src")).toBe(retained?.url);
    expect(native.request).not.toHaveBeenCalled();
    expect(native.renew).not.toHaveBeenCalled();
  } finally {
    await act(async () => mounted.unmount());
    client.clear();
  }
});

it("does not serve old pixels for a modified asset and revokes replaced Blob URLs", async () => {
  const before = await captureFolderThumbnail(asset("changed"), input());
  const modified = asset("changed", 2);
  expect(getFolderThumbnail(modified)).toBeUndefined();
  const after = await captureFolderThumbnail(modified, input());
  expect(getFolderThumbnail(modified)).toBe(after);
  expect(revokeUrl).toHaveBeenCalledWith(before?.url);
  discardFolderThumbnail(modified.path);
  expect(getFolderThumbnail(modified)).toBeUndefined();
  expect(revokeUrl).toHaveBeenCalledWith(after?.url);
});

it("uses the retained HIF loupe base even after its native thumbnail descriptor expires", async () => {
  const file = asset("loupe-base");
  const retained = await captureFolderThumbnail(file, input(120, 160));
  useImageProjectionStore.getState().accept({
    path: file.path, sourceRevision: "source", stateRevision: 1, validAt: 1,
    status: "ready", level: "thumbnail",
    result: { path: "/expired.jpg", width: 120, height: 160, kind: "embedded", renderLevel: "thumbnail",
      resource: { resourceId: "expired", url: "oxy-media://localhost/resource/expired", mediaType: "image/jpeg" } },
  });
  const client = new QueryClient();
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => root.render(<QueryClientProvider client={client}><Thumbnail asset={file} large /></QueryClientProvider>));
    expect(container.querySelectorAll("img")).toHaveLength(1);
    expect(container.querySelector("img")?.getAttribute("src")).toBe(retained?.url);
    expect(container.querySelector(".thumbnail__pending-image")).toBeNull();
    expect(native.request).not.toHaveBeenCalled();
    expect(native.renew).not.toHaveBeenCalled();
  } finally {
    await act(async () => root.unmount());
    client.clear();
    useImageProjectionStore.getState().clear();
  }
});

it("releases the previous folder and rejects a snapshot finishing after invalidation", async () => {
  setBrowserImageResourceScope("directory-a");
  const file = asset("late");
  const ready = await captureFolderThumbnail(asset("ready"), input());
  let finish: BlobCallback | undefined;
  vi.mocked(HTMLCanvasElement.prototype.toBlob).mockImplementation((callback) => { finish = callback; });
  const pending = captureFolderThumbnail(file, input());
  setBrowserImageResourceScope("directory-b");
  finish?.(new Blob(["late"]));
  expect(await pending).toBeUndefined();
  expect(getFolderThumbnailStats().count).toBe(0);
  expect(ready?.image.src).toBe("");
  expect(revokeUrl).toHaveBeenCalledWith(ready?.url);
});
