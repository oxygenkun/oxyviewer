// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { AssetBrowser } from "./AssetBrowser";
import { useWorkspaceStore } from "../store";
import type { AssetSummary, ViewMode } from "../types";

interface VirtualOptions {
  count: number;
  estimateSize: (index: number) => number;
  paddingStart?: number;
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
      const { count, estimateSize } = virtual.options;
      const size = estimateSize(0);
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

vi.mock("../lib/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../lib/api")>();
  return {
    ...original,
    requestMetadata: vi.fn(() => Promise.resolve([])),
    reconcilePreviewSchedule: vi.fn(() => Promise.resolve()),
    releasePreviewSchedule: vi.fn(() => Promise.resolve()),
    upsertPreviewSchedule: vi.fn(() => Promise.resolve()),
    getExternalAppSettings: vi.fn(() => Promise.resolve({ apps: [], defaultAppId: null })),
  };
});

vi.mock("./Thumbnail", () => ({ Thumbnail: () => null }));

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

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

// 24 photos: landscape columns are 4 wide at the default 900px width, portrait 6.
const assets = Array.from({ length: 24 }, (_, index) => asset(index));

let root: Root;
let host: HTMLDivElement;
let client: QueryClient;

const render = async (view: ViewMode) => {
  await act(async () => root.render(
    <QueryClientProvider client={client}>
      <AssetBrowser
        assets={assets}
        total={assets.length}
        hasNextPage={false}
        isFetchingNextPage={false}
        fetchNextPage={vi.fn()}
        onTrashAsset={vi.fn()}
        onCopyAssetPath={vi.fn()}
        onOpenInFileManager={vi.fn()}
        onOpenExternal={vi.fn()}
        deletionMode="trash"
        view={view}
        t={(key) => key}
      />
    </QueryClientProvider>,
  ));
};

const select = async (id: string) => {
  await act(async () => useWorkspaceStore.getState().select(id));
};

const switchOrientation = async (orientation: "landscape" | "portrait") => {
  await act(async () => useWorkspaceStore.getState().setThumbnailOrientation(orientation));
};

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", ResizeObserverStub);
  virtual.measure.mockClear();
  virtual.scrollToIndex.mockClear();
  useWorkspaceStore.setState({
    view: "grid",
    thumbnailOrientation: "landscape",
    activeId: undefined,
    selectedIds: [],
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

it("centers the restored photo once and leaves selection alone", async () => {
  useWorkspaceStore.setState({ activeId: "a0", selectedIds: ["a0"] });
  await render("grid");
  expect(virtual.scrollToIndex).toHaveBeenCalledWith(0, { align: "center" });
  virtual.scrollToIndex.mockClear();
  await select("a20");
  expect(virtual.scrollToIndex).not.toHaveBeenCalled();
});

it("keeps the selected photo in view when the grid orientation changes", async () => {
  // a8 restores to landscape row 2 and would move to portrait row 1, so a
  // re-run of the restore would hijack the viewport from the new selection.
  useWorkspaceStore.setState({ activeId: "a8", selectedIds: ["a8"] });
  await render("grid");
  expect(virtual.scrollToIndex).toHaveBeenCalledWith(2, { align: "center" });
  await select("a20");
  virtual.scrollToIndex.mockClear();
  virtual.measure.mockClear();

  await switchOrientation("portrait");

  // a20 moves from landscape row 5 (4 columns) to portrait row 3 (6 columns),
  // and the restored photo no longer re-centers the viewport.
  expect(virtual.measure).toHaveBeenCalled();
  expect(virtual.scrollToIndex).toHaveBeenCalledWith(3, { align: "auto" });
  expect(virtual.scrollToIndex).not.toHaveBeenCalledWith(expect.anything(), { align: "center" });
});

it("keeps the selected photo in view when the list orientation changes", async () => {
  useWorkspaceStore.setState({ activeId: "a20", selectedIds: ["a20"] });
  await render("list");
  // The frozen column header is reserved inside the virtualizer, not as scroll
  // container padding, so its offsets match the scroll element.
  expect(virtual.options.paddingStart).toBe(29);
  expect(virtual.scrollToIndex).toHaveBeenCalledWith(20, { align: "center" });
  virtual.scrollToIndex.mockClear();
  virtual.measure.mockClear();

  await switchOrientation("portrait");

  expect(virtual.measure).toHaveBeenCalled();
  expect(virtual.scrollToIndex).toHaveBeenCalledWith(20, { align: "auto" });
});
