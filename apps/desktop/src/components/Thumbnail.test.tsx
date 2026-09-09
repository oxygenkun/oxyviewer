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
import { acceptImageProjection, clearImageProjections } from "../lib/imageProjection";
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
});

describe("filmstrip thumbnail display retention", () => {
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

  it("leases only displayed and pending, then releases preview after full loads", async () => {
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
    await act(async () => projectStage("preview"));
    expect(apiMocks.renewMediaResource).toHaveBeenCalledWith("thumbnail");
    expect(apiMocks.renewMediaResource).toHaveBeenCalledWith("preview");
    expect(apiMocks.releaseMediaResource).not.toHaveBeenCalledWith("thumbnail");
    await act(async () => container.querySelector(".thumbnail__pending-image")!.dispatchEvent(new Event("load")));
    expect(apiMocks.releaseMediaResource).toHaveBeenCalledWith("thumbnail");
    await act(async () => projectStage("full"));
    expect(apiMocks.releaseMediaResource).not.toHaveBeenCalledWith("preview");
    await act(async () => container.querySelector(".thumbnail__pending-image")!.dispatchEvent(new Event("load")));
    expect(apiMocks.releaseMediaResource).toHaveBeenCalledWith("preview");
    apiMocks.renewMediaResource.mockClear();
    await act(async () => projectStage("thumbnail", 2));
    expect(apiMocks.renewMediaResource).not.toHaveBeenCalledWith("thumbnail");
    expect(apiMocks.renewMediaResource).not.toHaveBeenCalledWith("preview");
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
