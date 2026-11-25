// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { App } from "./App";
import { listAssets, listLibraryRoots } from "@/lib/api";
import { filterAndSortAssets } from "@/lib/assets/assetFiltering";
import { completeFolderOnboarding } from "@/lib/browse/workspacePersistence";
import type { RawPreviewStatus } from "@/lib/media/rawPreview";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary } from "@/types";

const rawRenderer = vi.hoisted(() => ({ report: undefined as ((status: RawPreviewStatus) => void) | undefined }));
vi.mock("@/lib/api", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/lib/api")>(),
  listAssets: vi.fn(),
  listLibraryRoots: vi.fn(),
  preloadAssetThumbnail: vi.fn(async () => undefined),
}));
vi.mock("@/components/browsing/Thumbnail", () => ({
  Thumbnail: ({ asset, onRawPreviewStatus }: { asset: AssetSummary; onRawPreviewStatus?: typeof rawRenderer.report }) => {
    if (asset.kind === "raw" && onRawPreviewStatus) rawRenderer.report = onRawPreviewStatus;
    return null;
  },
}));

it("shares mixed-format results and metadata filters between Grid and Loupe", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
  const directory = "/demo/Field Notes";
  const makeAsset = (name: string, kind: AssetSummary["kind"]): AssetSummary => ({
    id: name, name, path: `${directory}/${name}`, extension: name.split(".")[1],
    kind, sizeBytes: 10, modifiedAtMs: 1, hasSidecar: false, rating: 3,
  });
  const png = makeAsset("image-1.png", "png");
  const jpeg = makeAsset("image-2.jpg", "jpeg");
  const raw = makeAsset("image-3.nef", "raw");
  const assets = [png, jpeg, raw, makeAsset("image-4.heic", "heif"),
    { ...makeAsset("image-5.jpg", "jpeg"), rating: 1 }];
  vi.mocked(listLibraryRoots).mockResolvedValue([directory]);
  vi.mocked(listAssets).mockImplementation(async (_session, _directory, query) => {
    const items = filterAndSortAssets(assets, query);
    return { items, total: items.length };
  });
  completeFolderOnboarding();
  const initialState = useWorkspaceStore.getState();
  useWorkspaceStore.setState({ locale: "en", view: "grid", search: "image", minimumRating: 3 });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const flushUpdates = () => act(async () => { await vi.advanceTimersByTimeAsync(20); });
  const statusPath = () => host.querySelector(".statusbar__asset-path");
  const status = () => host.querySelector(".statusbar__preview");
  const expectFilters = () => {
    const formatButtons = [...host.querySelectorAll(".filterbar button")]
      .filter((button) => /^(All|RAW|JPEG|HEIF)$/.test(button.textContent?.trim() ?? ""));
    expect(formatButtons).toHaveLength(0);
    expect(host.querySelector('button[aria-label="3★+"]')?.getAttribute("aria-pressed")).toBe("true");
    expect(host.querySelector(".statusbar")?.textContent).toContain("/ 4 photos");
  };

  try {
    await act(async () => root.render(<QueryClientProvider client={client}><App /></QueryClientProvider>));
    await flushUpdates();
    await flushUpdates();
    const gridButton = host.querySelector<HTMLButtonElement>('button[aria-label="Grid"]')!;
    const loupeButton = host.querySelector<HTMLButtonElement>('button[aria-label="Loupe"]')!;
    expect(host.querySelector('.toolbar [role="combobox"]')).toBeTruthy();
    expect(host.querySelector(".toolbar")?.textContent).not.toMatch(/Workspace|4 photos/);
    expectFilters();
    expect(statusPath()?.textContent).toBe(directory);
    await act(async () => useWorkspaceStore.getState().select(jpeg.id));
    expect(statusPath()?.textContent).toBe(jpeg.path);
    await act(async () => loupeButton.click());

    expectFilters();
    expect(statusPath()?.textContent).toBe(jpeg.path);
    expect(host.querySelector(".loupe__caption")?.textContent).not.toContain(jpeg.name);
    expect(host.querySelector(".loupe__caption")?.textContent).not.toContain("KB");
    await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", code: "ArrowLeft" })));
    expect(statusPath()?.textContent).toBe(png.path);

    await act(async () => useWorkspaceStore.getState().select(raw.id));
    expect(statusPath()?.textContent).toBe(raw.path);
    expect(statusPath()?.nextElementSibling).toBe(status());
    expect(status()?.closest("footer.statusbar")).toBeTruthy();
    expect(status()?.textContent).toBe("Loading preview");
    await act(async () => rawRenderer.report!({ state: "fullReady", width: 6000, height: 4000 }));
    expect(status()?.textContent).toBe("Full RAW ready · 6000 × 4000");
    expect(host.querySelector(".loupe__stage")?.textContent).not.toContain("Full RAW ready");
    await act(async () => rawRenderer.report!({ state: "fullFailed" }));
    expect(status()?.textContent).toBe("Full RAW failed · showing preview");
    await act(async () => useWorkspaceStore.getState().select(jpeg.id));
    expect(statusPath()?.textContent).toBe(jpeg.path);
    expect(status()?.textContent).toBe("");
    await act(async () => useWorkspaceStore.getState().select(raw.id));
    expect(status()?.textContent).toBe("Loading preview");

    await act(async () => gridButton.click());
    expectFilters();
    expect(statusPath()?.textContent).toBe(raw.path);
    expect(status()?.textContent).toBe("");
    const searches = vi.mocked(listAssets).mock.calls.filter(([, , query]) => query.search);
    expect(searches).toHaveLength(1);
    expect(searches[0][2]).toMatchObject({ search: "image", minimumRating: 3 });
    expect(searches[0][2]).not.toHaveProperty("kind");
  } finally {
    await act(async () => root.unmount());
    host.remove();
    client.clear();
    useWorkspaceStore.setState(initialState);
    vi.unstubAllGlobals();
    vi.useRealTimers();
  }
});
