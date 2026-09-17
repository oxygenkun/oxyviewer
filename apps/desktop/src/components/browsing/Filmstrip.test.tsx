// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Filmstrip } from "./Filmstrip";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary } from "@/types";

interface VirtualOptions {
  count: number;
  estimateSize: (index: number) => number;
  gap?: number;
}

const virtual = vi.hoisted(() => ({
  options: undefined as unknown as VirtualOptions,
  measure: vi.fn(),
  scrollToIndex: vi.fn(),
}));

vi.mock("@tanstack/react-virtual", () => {
  // Mirror the real hook: the instance is stable while its options change per render.
  const instance = {
    getVirtualItems: () => {
      const { count, estimateSize, gap = 0 } = virtual.options;
      const size = estimateSize(0) + gap;
      return Array.from({ length: count }, (_, index) => ({
        index, key: index, start: index * size, end: (index + 1) * size, size, lane: 0,
      }));
    },
    getTotalSize: () => virtual.options.count * virtual.options.estimateSize(0),
    measure: virtual.measure,
    scrollToIndex: virtual.scrollToIndex,
  };
  return {
    useVirtualizer: (options: VirtualOptions) => {
      virtual.options = options;
      return instance;
    },
  };
});

vi.mock("@/lib/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...original,
    requestMetadata: vi.fn(() => Promise.resolve([])),
    getBatchedAssetTags: vi.fn(() => Promise.resolve([])),
  };
});

vi.mock("./Thumbnail", () => ({ Thumbnail: () => null }));
vi.mock("@/components/loupe/FilmstripPreviewPreloader", () => ({ FilmstripPreviewPreloader: () => null }));

const asset = (index: number): AssetSummary => ({
  id: `a${index}`,
  path: `/demo/a${index}.jpg`,
  name: `a${index}.jpg`,
  extension: "jpg",
  kind: "jpeg",
  sizeBytes: 1024,
  modifiedAtMs: 1,
  hasSidecar: false,
});

const assets = Array.from({ length: 40 }, (_, index) => asset(index));

let root: Root;
let host: HTMLDivElement;
let client: QueryClient;

const render = async (active: AssetSummary) => {
  await act(async () => root.render(
    <QueryClientProvider client={client}>
      <Filmstrip
        active={active}
        assets={assets}
        nearbyPreviewAssets={[]}
        total={assets.length}
        fetchNextPage={vi.fn()}
        hasNextPage={false}
        isFetchingNextPage={false}
        onAssetContextMenu={vi.fn()}
      />
    </QueryClientProvider>,
  ));
};

const switchOrientation = async (orientation: "landscape" | "portrait") => {
  await act(async () => useWorkspaceStore.getState().setThumbnailOrientation(orientation));
};

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  virtual.measure.mockClear();
  virtual.scrollToIndex.mockClear();
  useWorkspaceStore.setState({
    view: "loupe",
    thumbnailOrientation: "landscape",
    activeId: "a20",
    selectedIds: ["a20"],
    settingsOpen: false,
  });
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  client.clear();
  vi.unstubAllGlobals();
});

it("keeps the active filmstrip item framed when the orientation changes", async () => {
  await render(assets[20]);
  expect(virtual.scrollToIndex).toHaveBeenCalledWith(20, { align: "auto" });
  virtual.scrollToIndex.mockClear();
  virtual.measure.mockClear();

  await switchOrientation("portrait");

  // Portrait items are narrower, so the same offset can push the active item out.
  expect(virtual.measure).toHaveBeenCalled();
  expect(virtual.scrollToIndex).toHaveBeenCalledWith(20, { align: "auto" });
});

it("frames the active item when the selection moves", async () => {
  await render(assets[20]);
  virtual.scrollToIndex.mockClear();

  await act(async () => useWorkspaceStore.getState().select("a30"));
  await render(assets[30]);

  expect(virtual.scrollToIndex).toHaveBeenCalledWith(30, { align: "auto" });
});
