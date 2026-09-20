// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { AssetBrowser } from "./AssetBrowser";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary, ViewMode } from "@/types";

interface VirtualOptions {
  count: number;
  estimateSize: (index: number) => number;
}

const virtual = vi.hoisted(() => ({
  options: undefined as unknown as VirtualOptions,
  measure: vi.fn(),
  scrollToIndex: vi.fn(),
}));

const api = vi.hoisted(() => ({
  scanBurstGroups: vi.fn(),
}));
const trashAssets = vi.fn();

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

vi.mock("@/lib/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...original,
    requestMetadata: vi.fn(() => Promise.resolve([])),
    scanBurstGroups: api.scanBurstGroups,
    reconcilePreviewSchedule: vi.fn(() => Promise.resolve()),
    releasePreviewSchedule: vi.fn(() => Promise.resolve()),
    upsertPreviewSchedule: vi.fn(() => Promise.resolve()),
    getExternalAppSettings: vi.fn(() => Promise.resolve({ apps: [], defaultAppId: null })),
  };
});

vi.mock("./Thumbnail", () => ({
  Thumbnail: ({ onContextMenu }: { onContextMenu?: React.MouseEventHandler<HTMLSpanElement> }) => (
    <span className="thumbnail" onContextMenu={onContextMenu} />
  ),
}));

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

const render = async (view: ViewMode, renderedAssets = assets) => {
  await act(async () => root.render(
    <QueryClientProvider client={client}>
      <AssetBrowser
        assets={renderedAssets}
        total={renderedAssets.length}
        hasNextPage={false}
        isFetchingNextPage={false}
        fetchNextPage={vi.fn()}
        onTrashAssets={trashAssets}
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
  api.scanBurstGroups.mockReset();
  api.scanBurstGroups.mockResolvedValue([]);
  trashAssets.mockReset();
  useWorkspaceStore.setState({
    view: "grid",
    thumbnailOrientation: "landscape",
    burstGroupingEnabled: true,
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

it("supports shift and control multi-selection while styling only the focus as active", async () => {
  await render("grid");
  const first = host.querySelector<HTMLButtonElement>('button[title="/demo/a1.jpg"]')!;
  const fourth = host.querySelector<HTMLButtonElement>('button[title="/demo/a4.jpg"]')!;
  const sixth = host.querySelector<HTMLButtonElement>('button[title="/demo/a6.jpg"]')!;

  await act(async () => first.click());
  await act(async () => fourth.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true })));
  expect(useWorkspaceStore.getState()).toMatchObject({
    selectedIds: ["a1", "a2", "a3", "a4"],
    activeId: "a4",
  });
  expect(first.classList.contains("is-selected")).toBe(true);
  expect(first.classList.contains("is-active")).toBe(false);
  expect(fourth.classList.contains("is-active")).toBe(true);

  await act(async () => sixth.dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true })));
  expect(useWorkspaceStore.getState().selectedIds).toEqual(["a1", "a2", "a3", "a4", "a6"]);
  expect(sixth.classList.contains("is-active")).toBe(true);
  expect(fourth.classList.contains("is-active")).toBe(false);
});

it("deletes the current multi-selection from the context menu", async () => {
  await render("grid");
  const first = host.querySelector<HTMLButtonElement>('button[title="/demo/a1.jpg"]')!;
  const second = host.querySelector<HTMLButtonElement>('button[title="/demo/a2.jpg"]')!;
  await act(async () => first.click());
  await act(async () => second.dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true })));
  await act(async () => first.querySelector(".thumbnail")?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true })));

  const deleteButton = [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
    .find((button) => button.textContent === "deleteSelected".replace("{count}", "2"));
  expect(deleteButton?.textContent).toBe("deleteSelected");
  await act(async () => deleteButton?.click());
  expect(document.querySelector('[role="alertdialog"]')?.textContent).toContain("trashConfirmMultipleBody");

  const confirm = [...document.querySelectorAll<HTMLButtonElement>(".trash-confirm-dialog button")]
    .find((button) => button.textContent === "confirmTrash");
  await act(async () => confirm?.click());
  expect(trashAssets).toHaveBeenCalledWith([assets[1], assets[2]]);
});

it("expands a collapsed burst from its cover and updates the virtual row count", async () => {
  api.scanBurstGroups.mockResolvedValue([{
    representative: "/demo/a0.jpg",
    members: Array.from({ length: 6 }, (_, index) => `/demo/a${index}.jpg`),
  }]);
  await render("grid");
  await act(async () => undefined);

  // Six frames collapse to one tile: 24 - 5 = 19 visible items, or five rows.
  expect(virtual.options.count).toBe(5);
  expect(host.querySelector('button[title="/demo/a1.jpg"]')).toBeNull();

  const cover = host.querySelector<HTMLButtonElement>('button[title="/demo/a0.jpg"]');
  expect(cover).not.toBeNull();
  const expandClick = new MouseEvent("click", { bubbles: true });
  Object.defineProperty(expandClick, "timeStamp", { value: 100 });
  await act(async () => cover?.dispatchEvent(expandClick));

  const initialDoubleClick = new MouseEvent("dblclick", { bubbles: true });
  Object.defineProperty(initialDoubleClick, "timeStamp", { value: 200 });
  await act(async () => cover?.dispatchEvent(initialDoubleClick));
  expect(useWorkspaceStore.getState().view).toBe("grid");

  expect(virtual.options.count).toBe(6);
  const secondFrame = host.querySelector<HTMLButtonElement>('button[title="/demo/a1.jpg"]');
  expect(secondFrame).not.toBeNull();
  expect(host.querySelectorAll(".asset-card.is-burst-expanded")).toHaveLength(6);
  await act(async () => secondFrame?.click());
  expect(useWorkspaceStore.getState().activeId).toBe("a1");

  virtual.scrollToIndex.mockClear();
  const collapse = secondFrame?.querySelector<HTMLElement>(".asset-card__burst");
  expect(collapse).not.toBeNull();
  await act(async () => collapse?.click());
  expect(virtual.options.count).toBe(5);
  expect(useWorkspaceStore.getState().activeId).toBe("a0");
  expect(virtual.scrollToIndex).toHaveBeenCalledWith(0, { align: "start" });
});

it("keeps the collapsed representative in place when it remains visible", async () => {
  api.scanBurstGroups.mockResolvedValue([{
    representative: "/demo/a0.jpg",
    members: Array.from({ length: 6 }, (_, index) => `/demo/a${index}.jpg`),
  }]);
  await render("grid");
  await act(async () => undefined);
  const scroll = host.querySelector<HTMLElement>(".asset-scroll");
  Object.defineProperty(scroll, "clientHeight", { value: 500 });

  const cover = host.querySelector<HTMLButtonElement>('button[title="/demo/a0.jpg"]');
  await act(async () => cover?.click());
  virtual.scrollToIndex.mockClear();
  const secondFrame = host.querySelector<HTMLButtonElement>('button[title="/demo/a1.jpg"]');
  const collapse = secondFrame?.querySelector<HTMLElement>(".asset-card__burst");
  await act(async () => collapse?.click());

  expect(virtual.scrollToIndex).not.toHaveBeenCalled();
});

it("opens loupe only when a burst was already expanded before the double click", async () => {
  api.scanBurstGroups.mockResolvedValue([{
    representative: "/demo/a0.jpg",
    members: ["/demo/a0.jpg", "/demo/a1.jpg"],
  }]);
  await render("grid");
  await act(async () => undefined);
  const cover = host.querySelector<HTMLButtonElement>('button[title="/demo/a0.jpg"]');

  const expandClick = new MouseEvent("click", { bubbles: true });
  Object.defineProperty(expandClick, "timeStamp", { value: 100 });
  await act(async () => cover?.dispatchEvent(expandClick));
  const expandedDoubleClick = new MouseEvent("dblclick", { bubbles: true });
  Object.defineProperty(expandedDoubleClick, "timeStamp", { value: 1_000 });
  await act(async () => cover?.dispatchEvent(expandedDoubleClick));

  expect(useWorkspaceStore.getState().view).toBe("loupe");
});

it("publishes the first burst groups before the remaining paths finish scanning", async () => {
  const manyAssets = Array.from({ length: 100 }, (_, index) => asset(index));
  let finishSecondBatch: ((groups: []) => void) | undefined;
  api.scanBurstGroups
    .mockResolvedValueOnce([{
      representative: "/demo/a0.jpg",
      members: ["/demo/a0.jpg", "/demo/a1.jpg"],
    }])
    .mockImplementationOnce(() => new Promise((resolve) => {
      finishSecondBatch = resolve;
    }));

  await render("grid", manyAssets);
  await act(async () => undefined);

  expect(api.scanBurstGroups.mock.calls[0]?.[0]).toHaveLength(24);
  expect(api.scanBurstGroups.mock.calls[1]?.[0]).toHaveLength(88);
  expect(host.querySelector('button[title="/demo/a1.jpg"]')).toBeNull();

  await act(async () => finishSecondBatch?.([]));
});
