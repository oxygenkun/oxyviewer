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

vi.mock("../lib/api", () => ({
  isTauri: () => false,
  generatedPreview: vi.fn(),
  previewUrl: (asset: AssetSummary) => asset.path,
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

function project(revision: number) {
  acceptImageProjection({
    path: asset.path, sourceRevision: "source-1", projectionRevision: revision,
    validAt: 1, status: "ready", level: "thumbnail",
    result: { path: url, width: 160, height: 120, kind: "embedded" },
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

  it("never carries the retained image over to another asset", async () => {
    markBrowserImageReady(url, { width: 160, height: 120 });
    await render("loupe");
    clearBrowserImageResources();
    await render("visible", { ...asset, id: "b", path: "/photos/b.hif" });
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector(".thumbnail__fallback")).not.toBeNull();
  });
});
