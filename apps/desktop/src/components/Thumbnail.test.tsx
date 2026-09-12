// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearBrowserImageResources,
  discardBrowserImageResource,
  markBrowserImageReady,
} from "../lib/browserImageCache";
import { acceptImageProjection, clearImageProjections, invalidateImageProjection } from "../lib/imageProjection";
import type { AssetSummary, PreviewPriority } from "../types";
import { Thumbnail } from "./Thumbnail";

const apiMocks = vi.hoisted(() => ({
  tauri: false,
  generatedPreview: vi.fn(),
  renewMediaResource: vi.fn((_id: string) => Promise.resolve(true)),
  releaseMediaResource: vi.fn(() => Promise.resolve()),
}));

vi.mock("../lib/api", () => ({
  isTauri: () => apiMocks.tauri,
  generatedPreview: apiMocks.generatedPreview,
  previewUrl: (asset: AssetSummary) => asset.path,
  renewMediaResource: apiMocks.renewMediaResource,
  releaseMediaResource: apiMocks.releaseMediaResource,
}));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (path: string) => path }));
// Canvas/Blob snapshots are covered by folderThumbnailCache tests and the real
// browser probe; these tests isolate the existing native-image lease lifecycle.
vi.mock("../lib/folderThumbnailCache", async (importOriginal) => ({
  ...await importOriginal<typeof import("../lib/folderThumbnailCache")>(),
  preloadFolderThumbnail: async () => undefined,
  getFolderThumbnail: () => undefined,
}));

const asset: AssetSummary = {
  id: "a", path: "/photos/a.hif", name: "a.hif", extension: "hif",
  kind: "heif", sizeBytes: 100, modifiedAtMs: 1, hasSidecar: false,
};
const url = "/cache/a.jpg";
let container: HTMLDivElement;
let root: Root;
let client: QueryClient;

function project(revision: number, resourceId?: string) {
  acceptImageProjection({
    path: asset.path, sourceRevision: "source-1", stateRevision: revision,
    validAt: 1, status: "ready", level: "thumbnail",
    result: {
      path: url,
      width: 160,
      height: 120,
      kind: "embedded",
      renderLevel: "thumbnail",
      ...(resourceId ? {
        resource: {
          resourceId,
          url: `oxy-media://localhost/resource/${resourceId}`,
          mediaType: "image/jpeg",
        },
      } : {}),
    },
  });
}

async function render(priority: PreviewPriority, currentAsset = asset, enabled = true) {
  await act(async () => {
    root.render(
      <StrictMode>
        <QueryClientProvider client={client}>
          <Thumbnail asset={currentAsset} priority={priority} enabled={enabled} />
        </QueryClientProvider>
      </StrictMode>,
    );
  });
}

beforeEach(() => {
  apiMocks.tauri = false;
  apiMocks.generatedPreview.mockReset();
  apiMocks.renewMediaResource.mockClear();
  apiMocks.releaseMediaResource.mockClear();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("__OXY_DEBUG__", false);
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows");
  clearImageProjections();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  project(1);
});

afterEach(async () => {
  await act(async () => root.unmount());
  client.clear();
  container.remove();
  clearImageProjections();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  delete (HTMLImageElement.prototype as Partial<HTMLImageElement>).decode;
});

describe("filmstrip thumbnail display retention", () => {
  it("keeps the displayed RAW full image through a retry and reports an eventual failure", async () => {
    apiMocks.tauri = true;
    clearImageProjections();
    const raw = { ...asset, kind: "raw" as const, path: "/photos/retry.arw" };
    const status = vi.fn();
    const fullUrl = "/cache/raw-full.jpg";
    markBrowserImageReady(fullUrl, { width: 4688, height: 7028 });
    acceptImageProjection({ path: raw.path, sourceRevision: "original", stateRevision: 1,
      validAt: 1, status: "ready", level: "full", result: { path: fullUrl, width: 4688, height: 7028,
        kind: "developed", renderLevel: "full", satisfaction: "satisfied" } });
    await act(async () => root.render(<QueryClientProvider client={client}>
      <Thumbnail asset={raw} large onRawPreviewStatus={status} />
    </QueryClientProvider>));
    expect(status).toHaveBeenLastCalledWith({ state: "fullReady", width: 4688, height: 7028 });
    await act(async () => invalidateImageProjection(raw.path, "full"));
    expect(container.querySelector(".thumbnail__displayed-image")?.getAttribute("src")
      ?? container.querySelector("img")?.getAttribute("src")).toBe(fullUrl);
    expect(status).toHaveBeenLastCalledWith({ state: "developingFull" });
    await act(async () => acceptImageProjection({ path: raw.path, sourceRevision: "retry", stateRevision: 2,
      validAt: 2, status: "error", level: "full", error: "decode failed" }));
    expect(status).toHaveBeenLastCalledWith({ state: "fullFailed" });
    expect(container.querySelector("img")?.getAttribute("src")).toBe(fullUrl);
  });
  it("keeps a cached RAW full Interim in developing state", async () => {
    apiMocks.tauri = true;
    clearImageProjections();
    const raw = { ...asset, kind: "raw" as const };
    const status = vi.fn();
    markBrowserImageReady(url, { width: 341, height: 512 });
    for (const level of ["thumbnail", "full"] as const) {
      acceptImageProjection({
        path: raw.path, sourceRevision: "source-1", stateRevision: 1,
        validAt: 1, status: "ready", level,
        result: { path: url, width: 341, height: 512, kind: "embedded",
          renderLevel: level, satisfaction: level === "full" ? "interim" : "satisfied" },
      });
    }
    await act(async () => root.render(<QueryClientProvider client={client}>
      <Thumbnail asset={raw} large onRawPreviewStatus={status} />
    </QueryClientProvider>));
    expect(status).toHaveBeenLastCalledWith({ state: "developingFull" });
    expect(status.mock.calls.some(([value]) => value.state === "fullReady")).toBe(false);
    clearBrowserImageResources();
  });

  it("retains an available full resource while the thumbnail is still loading", async () => {
    apiMocks.tauri = true;
    clearImageProjections();
    const jpeg = { ...asset, kind: "jpeg" as const };
    for (const level of ["thumbnail", "full"] as const) {
      acceptImageProjection({
        path: jpeg.path, sourceRevision: "source-1", stateRevision: 1,
        validAt: 1, status: "ready", level,
        result: {
          path: `/cache/${level}.jpg`, width: level === "full" ? 7008 : 342,
          height: level === "full" ? 4672 : 512, kind: "embedded", renderLevel: level,
          resource: { resourceId: `awaiting-${level}`, url: `oxy-media://localhost/resource/awaiting-${level}`, mediaType: "image/jpeg" },
        },
      });
    }
    await act(async () => root.render(<QueryClientProvider client={client}>
      <Thumbnail asset={jpeg} large />
    </QueryClientProvider>));
    expect(container.querySelector(".thumbnail__pending-image")?.getAttribute("src")).toContain("awaiting-thumbnail");
    expect(apiMocks.releaseMediaResource).not.toHaveBeenCalledWith("awaiting-full");
    expect(apiMocks.renewMediaResource).toHaveBeenCalledWith("awaiting-full");
  });

  it("reveals loupe only after decoding and ignores an unmounted late decode", async () => {
    apiMocks.tauri = true;
    clearImageProjections();
    const ready: (() => void)[] = [];
    Object.defineProperty(HTMLImageElement.prototype, "decode", {
      configurable: true,
      value: vi.fn(() => new Promise<void>((resolve) => ready.push(resolve))),
    });
    const loaded = vi.fn();
    const mount = async (id: string) => act(async () => {
      acceptImageProjection({ path: `/photos/${id}.jpg`, sourceRevision: id, stateRevision: 1,
        validAt: 1, status: "ready", level: "full", result: {
          path: `/photos/${id}.jpg`, width: 2400, height: 1600, kind: "original", renderLevel: "full",
        } });
      root.render(
      <QueryClientProvider client={client}>
        <Thumbnail asset={{ ...asset, id, kind: "jpeg", path: `/photos/${id}.jpg` }} large onImageLoad={loaded} />
      </QueryClientProvider>,
      );
    });
    await mount("old");
    const old = container.querySelector(".thumbnail__pending-image")!;
    expect(old.getAttribute("decoding")).toBe("async");
    await act(async () => old.dispatchEvent(new Event("load")));
    expect(loaded).not.toHaveBeenCalled();
    await mount("new");
    const current = container.querySelector(".thumbnail__pending-image")!;
    await act(async () => current.dispatchEvent(new Event("load")));
    await act(async () => ready[0]());
    expect(loaded).not.toHaveBeenCalled();
    await act(async () => ready[1]());
    expect(loaded).toHaveBeenCalled();
    expect(container.querySelector("img")?.getAttribute("src")).toBe("/photos/new.jpg");
    delete (HTMLImageElement.prototype as Partial<HTMLImageElement>).decode;
  });
  it("starts loupe full without a thumbnail and aborts each previous selection", async () => {
    apiMocks.tauri = true;
    clearImageProjections();
    apiMocks.generatedPreview.mockImplementation((_asset: AssetSummary, _level: string, signal: AbortSignal) =>
      new Promise((_resolve, reject) => signal.addEventListener("abort", () => reject(signal.reason), { once: true })));
    const mountLoupe = async (id: string) => {
      await act(async () => root.render(<QueryClientProvider client={client}>
        <Thumbnail asset={{ ...asset, id, path: `/photos/${id}.arw`, kind: "raw" }} large />
      </QueryClientProvider>));
    };
    for (const id of ["a", "b", "c", "a"]) {
      await mountLoupe(id);
      const calls = apiMocks.generatedPreview.mock.calls;
      expect(calls.length).toBeGreaterThan(0);
      expect(calls.every((call) => call[1] === "full")).toBe(true);
      expect(calls.at(-1)![0].id).toBe(id);
      expect(calls.at(-1)![2].aborted).toBe(false);
      expect(calls.slice(0, -1).every((call) => call[2].aborted)).toBe(true);
    }
  });

  it("uses an existing HEIF thumbnail base without issuing a thumbnail request", async () => {
    apiMocks.tauri = true;
    markBrowserImageReady(url, { width: 160, height: 120 });
    await act(async () => root.render(<QueryClientProvider client={client}>
      <Thumbnail asset={asset} large />
    </QueryClientProvider>));
    expect(container.querySelector("img")?.getAttribute("src")).toBe(url);
    // HEIF full is owned by HeifTileCanvas; its base is a read-only observer.
    expect(apiMocks.generatedPreview).not.toHaveBeenCalled();
  });

  it("retains the displayed geometry until replacement pixels load, including a cache remount", async () => {
    const geometry = {
      displaySize: { width: 7008, height: 4672 },
      contentRect: { x: 0, y: 7, width: 160, height: 106 },
    };
    const publish = (revision: number, path: string, padded: boolean) => acceptImageProjection({
      path: asset.path, sourceRevision: "source-1", stateRevision: revision,
      validAt: 1, status: "ready", level: "thumbnail",
      result: { path, width: 160, height: 120, kind: "embedded", renderLevel: "thumbnail",
        geometry: padded ? geometry : undefined },
    });
    publish(2, url, true);
    markBrowserImageReady(url, { width: 160, height: 120 });
    await render("loupe");
    const content = container.querySelector<HTMLImageElement>(".thumbnail__content img")!;
    expect(content.src).toContain(url);
    expect(parseFloat(content.style.top)).toBeCloseTo(-7 / 106 * 100);
    // A new projection describes different pixels. The retained image must
    // keep its crop while that replacement remains hidden and undecoded.
    await act(async () => publish(3, "/cache/replacement.jpg", false));
    expect(container.querySelector(".thumbnail__content img")).toBe(content);
    const pending = container.querySelector<HTMLImageElement>(".thumbnail__pending-image")!;
    Object.defineProperties(pending, { naturalWidth: { value: 160 }, naturalHeight: { value: 120 } });
    await act(async () => pending.dispatchEvent(new Event("load")));
    expect(container.querySelector(".thumbnail__content")).toBeNull();
    expect(container.querySelector("img")?.src).toContain("/cache/replacement.jpg");
    await act(async () => root.render(null));
    await render("visible");
    expect(container.querySelector(".thumbnail__content")).toBeNull();
    expect(container.querySelector("img")?.src).toContain("/cache/replacement.jpg");
  });

  it("does not hide a cached image when deselection follows a projection update", async () => {
    markBrowserImageReady(url, { width: 160, height: 120 });
    await render("loupe");
    const image = container.querySelector("img");
    expect(image?.className).not.toContain("thumbnail__pending-image");

    // A newer projection invalidates the shared browser cache. The mounted
    // image is already visible and must not depend on another load event.
    await act(async () => project(2));
    await render("visible");
    expect(container.querySelector("img")).toBe(image);
    expect(image?.className).not.toContain("thumbnail__pending-image");
    expect(container.querySelector(".thumbnail__fallback")).toBeNull();
  });

  it("paints a decoded image immediately when a virtual item remounts during scrolling", async () => {
    markBrowserImageReady(url, { width: 160, height: 120 });
    await render("nearby", asset, false);

    const image = container.querySelector("img");
    expect(image?.getAttribute("src")).toBe(url);
    expect(image?.className).not.toContain("thumbnail__pending-image");
    expect(container.querySelector(".thumbnail__fallback")).toBeNull();
  });

  it("retains a prepared image after cache eviction while loading is disabled", async () => {
    markBrowserImageReady(url, { width: 160, height: 120 });
    await render("loupe");
    const image = container.querySelector("img");
    discardBrowserImageResource(url);
    await render("nearby", asset, false);
    expect(container.querySelector("img")).toBe(image);
    expect(container.querySelector(".thumbnail__fallback")).toBeNull();
  });

  it("still promotes a cold pending image and retains it across priority changes", async () => {
    await render("visible");
    const image = container.querySelector("img");
    expect(image?.className).toContain("thumbnail__pending-image");
    await act(async () => image?.dispatchEvent(new Event("load")));
    expect(container.querySelector(".thumbnail__fallback")).toBeNull();
    expect(image?.className).not.toContain("thumbnail__pending-image");
    discardBrowserImageResource(url);
    await render("loupe");
    await render("visible");
    expect(container.querySelector("img")).toBe(image);
    expect(image?.className).not.toContain("thumbnail__pending-image");
  });

  it("renews the active resource and releases it when the projection changes", async () => {
    project(2, "resource-1");
    await render("visible");
    expect(apiMocks.renewMediaResource).toHaveBeenCalledWith("resource-1");

    await act(async () => project(3, "resource-2"));
    await render("visible");
    expect(apiMocks.releaseMediaResource).toHaveBeenCalledWith("resource-1");
    expect(apiMocks.renewMediaResource).toHaveBeenCalledWith("resource-2");
  });

  it("leases the thumbnail base until full loads", async () => {
    const raw = { ...asset, kind: "raw" as const, path: "/photos/a.arw" };
    const projectStage = (level: "thumbnail" | "preview" | "full", revision = 1) => {
      acceptImageProjection({
        path: raw.path, sourceRevision: "source", stateRevision: revision,
        validAt: 1, status: "ready", level,
        result: { path: `/cache/${level}.jpg`, width: 100, height: 100,
          kind: "embedded", renderLevel: level,
          resource: { resourceId: level, url: `oxy-media://localhost/resource/${level}`, mediaType: "image/jpeg" },
        },
      });
    };
    projectStage("thumbnail");
    const mount = () => root.render(<StrictMode><QueryClientProvider client={client}>
      <Thumbnail asset={raw} large />
    </QueryClientProvider></StrictMode>);
    await act(async () => mount());
    await act(async () => container.querySelector(".thumbnail__pending-image")!.dispatchEvent(new Event("load")));
    expect(apiMocks.renewMediaResource).toHaveBeenCalledWith("thumbnail");
    await act(async () => projectStage("full"));
    expect(apiMocks.renewMediaResource).toHaveBeenCalledWith("full");
    expect(apiMocks.releaseMediaResource).not.toHaveBeenCalledWith("thumbnail");
    await act(async () => container.querySelector(".thumbnail__pending-image")!.dispatchEvent(new Event("load")));
    expect(apiMocks.releaseMediaResource).not.toHaveBeenCalledWith("thumbnail");
    apiMocks.renewMediaResource.mockClear();
    await act(async () => projectStage("thumbnail", 2));
    expect(apiMocks.renewMediaResource).not.toHaveBeenCalledWith("thumbnail");
    expect(container.querySelectorAll("img")).toHaveLength(1);
  });

  it("preserves the displayed resource ID across projection replacement until load", async () => {
    project(2, "old");
    await render("visible");
    await act(async () => container.querySelector("img")!.dispatchEvent(new Event("load")));
    await act(async () => project(3, "new"));
    expect(apiMocks.releaseMediaResource).not.toHaveBeenCalledWith("old");
    expect(apiMocks.renewMediaResource).toHaveBeenCalledWith("new");
    await act(async () => container.querySelector(".thumbnail__pending-image")!.dispatchEvent(new Event("load")));
    expect(apiMocks.releaseMediaResource).not.toHaveBeenCalledWith("old");
    await act(async () => clearBrowserImageResources());
    expect(apiMocks.releaseMediaResource).toHaveBeenCalledWith("old");
  });

  it("refetches an expired descriptor once and renews active images every ten seconds", async () => {
    vi.useFakeTimers();
    try {
      project(2, "stale");
      apiMocks.renewMediaResource.mockImplementation((id) => Promise.resolve(id !== "stale"));
      apiMocks.generatedPreview.mockImplementation(async () => { project(3, "fresh"); });
      await render("visible");
      expect(apiMocks.generatedPreview).toHaveBeenCalledTimes(1);
      expect(client.getQueryCache().getAll().some((query) => query.state.status === "error")).toBe(false);
      await act(async () => vi.advanceTimersByTimeAsync(10_000));
      expect(apiMocks.renewMediaResource).toHaveBeenCalledWith("fresh");
      expect(apiMocks.generatedPreview).toHaveBeenCalledTimes(1);
      expect(container.querySelector("img")?.getAttribute("src")).toContain("fresh");
    } finally { vi.useRealTimers(); apiMocks.renewMediaResource.mockResolvedValue(true); }
  });

  it("aborts a pending Interim upgrade when its TanStack subscriber unmounts", async () => {
    apiMocks.tauri = true;
    const signals: AbortSignal[] = [];
    apiMocks.generatedPreview.mockImplementation((
      requested: AssetSummary,
      _level: string,
      signal: AbortSignal,
    ) => {
      signals.push(signal);
      if (requested.id === asset.id) {
        acceptImageProjection({
          path: requested.path,
          sourceRevision: "source-1",
          stateRevision: 2,
          validAt: 1,
          status: "ready",
          level: "thumbnail",
          result: {
            path: url,
            width: 160,
            height: 120,
            kind: "embedded",
            renderLevel: "thumbnail",
            satisfaction: "interim",
            resource: {
              resourceId: "resource-interim",
              url: "oxy-media://localhost/resource/resource-interim",
              mediaType: "image/jpeg",
            },
          },
        });
      }
      return new Promise((_resolve, reject) => {
        signal.addEventListener("abort", () => reject(signal.reason), { once: true });
      });
    });

    await render("visible");
    expect(signals.some((signal) => !signal.aborted)).toBe(true);
    await render("visible", { ...asset, id: "b", path: "/photos/b.hif" });

    expect(signals.filter((_, index) => index === 0 || index === 1)
      .some((signal) => signal.aborted)).toBe(true);
  });

  it("never carries the retained image over to another asset", async () => {
    markBrowserImageReady(url, { width: 160, height: 120 });
    await render("loupe");
    clearBrowserImageResources();
    await render("visible", { ...asset, id: "b", path: "/photos/b.hif" });
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector(".thumbnail__fallback")).not.toBeNull();
  });
});
