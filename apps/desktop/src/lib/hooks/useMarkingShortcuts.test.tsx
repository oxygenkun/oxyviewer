// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { getAssetTagAssignmentsByPath, patchMetadata, setAssetCustomTag } from "@/lib/api";
import { useMetadataProjectionStore } from "@/lib/projection/metadataProjection";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary } from "@/types";
import { useMarkingShortcuts } from "./useMarkingShortcuts";

vi.mock("@/lib/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...original,
    patchMetadata: vi.fn().mockResolvedValue("job-1"),
    getAssetTagAssignmentsByPath: vi.fn(),
    setAssetCustomTag: vi.fn(),
  };
});

const asset = (id: string, overrides: Partial<AssetSummary> = {}): AssetSummary => ({
  id,
  path: `/demo/${id}`,
  name: id,
  extension: "ARW",
  kind: "raw",
  sizeBytes: 1024,
  modifiedAtMs: 1,
  hasSidecar: false,
  ...overrides,
});

const assets = [asset("a.arw"), asset("b.arw"), asset("c.arw")];

function Probe({ suppressed = false, items = assets, fallback }: {
  suppressed?: boolean; items?: AssetSummary[]; fallback?: AssetSummary;
}) {
  useMarkingShortcuts(items, suppressed, fallback);
  return null;
}

let root: Root;
let host: HTMLDivElement;
let client: QueryClient;
let tagged: Set<string>;

const pressKey = async (key: string, init: KeyboardEventInit = {}, target: EventTarget = window) => {
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));
  });
};

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.mocked(patchMetadata).mockClear();
  tagged = new Set();
  vi.mocked(getAssetTagAssignmentsByPath).mockReset().mockImplementation(async (paths) => paths.map((path) => ({
    path,
    assignments: tagged.has(path) ? [{
      tag: { id: 7, name: "Family", path: "People|Family", sortOrder: 0 }, assignedCount: 1, assetCount: 1,
    }] : [],
  })));
  vi.mocked(setAssetCustomTag).mockReset().mockImplementation(async (paths, _tagId, assigned) => {
    for (const path of paths) {
      if (assigned) tagged.add(path); else tagged.delete(path);
    }
  });
  window.localStorage.clear();
  useWorkspaceStore.getState().resetShortcuts();
  useWorkspaceStore.setState({ selectedIds: ["a.arw"], activeId: "a.arw", settingsOpen: false, lastTagId: 7 });
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root.render(
    <QueryClientProvider client={client}><Probe /></QueryClientProvider>,
  ));
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  client.clear();
  vi.unstubAllGlobals();
});

it("applies the rating to selected assets", async () => {
  await pressKey("3");
  expect(patchMetadata).toHaveBeenCalledWith(["/demo/a.arw"], { rating: 3 });
});

it("clears the rating with 0 and backquote", async () => {
  await pressKey("0");
  expect(patchMetadata).toHaveBeenCalledWith(["/demo/a.arw"], { rating: null });
  await pressKey("`");
  expect(patchMetadata).toHaveBeenCalledTimes(2);
});

it("toggles a color label off when already active", async () => {
  await act(async () => useMetadataProjectionStore.setState({
    records: { "/demo/a.arw": { path: "/demo/a.arw", colorLabel: "Red", status: "ready" } as never },
  }));
  await pressKey("6");
  expect(patchMetadata).toHaveBeenCalledWith(["/demo/a.arw"], { colorLabel: null });
});

it("does nothing when suppressed", async () => {
  await act(async () => root.unmount());
  root = createRoot(host);
  await act(async () => root.render(
    <QueryClientProvider client={client}><Probe suppressed /></QueryClientProvider>,
  ));
  await pressKey("3");
  await pressKey("t", { ctrlKey: true });
  expect(patchMetadata).not.toHaveBeenCalled();
  expect(setAssetCustomTag).not.toHaveBeenCalled();
});

it("adds the last tag to a new photo and removes it on the next press", async () => {
  await pressKey("t", { ctrlKey: true });
  await vi.waitFor(() => expect(tagged.has("/demo/a.arw")).toBe(true));
  await pressKey("t", { ctrlKey: true });
  await vi.waitFor(() => expect(setAssetCustomTag).toHaveBeenLastCalledWith(["/demo/a.arw"], 7, false));
  expect(tagged.size).toBe(0);
  expect(patchMetadata).not.toHaveBeenCalled();
});

it("toggles each selected photo independently and leaves unselected photos alone", async () => {
  tagged.add("/demo/a.arw");
  tagged.add("/demo/c.arw");
  await act(async () => useWorkspaceStore.setState({ selectedIds: ["a.arw", "b.arw"] }));
  await pressKey("t", { ctrlKey: true });
  await vi.waitFor(() => expect(setAssetCustomTag).toHaveBeenCalledTimes(2));
  expect(setAssetCustomTag).toHaveBeenCalledWith(["/demo/a.arw"], 7, false);
  expect(setAssetCustomTag).toHaveBeenCalledWith(["/demo/b.arw"], 7, true);
  expect([...tagged]).toEqual(["/demo/c.arw", "/demo/b.arw"]);
});

it("keeps the remembered tag when moving to another photo", async () => {
  await pressKey("t", { ctrlKey: true });
  await vi.waitFor(() => expect(setAssetCustomTag).toHaveBeenCalledTimes(1));
  await act(async () => useWorkspaceStore.getState().select("b.arw"));
  await pressKey("t", { ctrlKey: true });
  await vi.waitFor(() => expect(setAssetCustomTag).toHaveBeenLastCalledWith(["/demo/b.arw"], 7, true));
});

it("serializes rapid presses so both toggles take effect", async () => {
  await act(async () => {
    for (let index = 0; index < 2; index += 1) {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "t", ctrlKey: true, cancelable: true }));
    }
  });
  await vi.waitFor(() => expect(setAssetCustomTag).toHaveBeenCalledTimes(2));
  expect(setAssetCustomTag).toHaveBeenNthCalledWith(1, ["/demo/a.arw"], 7, true);
  expect(setAssetCustomTag).toHaveBeenNthCalledWith(2, ["/demo/a.arw"], 7, false);
});

it("uses the active fallback when the loupe retains a photo outside the result list", async () => {
  await act(async () => root.render(
    <QueryClientProvider client={client}><Probe items={[]} fallback={assets[0]} /></QueryClientProvider>,
  ));
  await pressKey("t", { ctrlKey: true });
  await vi.waitFor(() => expect(setAssetCustomTag).toHaveBeenCalledWith(["/demo/a.arw"], 7, true));
});

it("consumes the shortcut without writing when no tag or target exists", async () => {
  useWorkspaceStore.setState({ lastTagId: undefined });
  const event = new KeyboardEvent("keydown", { key: "t", ctrlKey: true, cancelable: true });
  await act(async () => { window.dispatchEvent(event); });
  expect(event.defaultPrevented).toBe(true);
  expect(getAssetTagAssignmentsByPath).not.toHaveBeenCalled();
  useWorkspaceStore.setState({ lastTagId: 7 });
  await act(async () => root.render(
    <QueryClientProvider client={client}><Probe items={[]} /></QueryClientProvider>,
  ));
  await pressKey("t", { ctrlKey: true });
  expect(setAssetCustomTag).not.toHaveBeenCalled();
});

it("honors a customized binding", async () => {
  await act(async () => useWorkspaceStore.getState().setShortcut("marking.repeatLastTag", 0,
    { key: "y", ctrl: false, alt: true, shift: false, meta: false }));
  await pressKey("t", { ctrlKey: true });
  await pressKey("t");
  expect(setAssetCustomTag).not.toHaveBeenCalled();
  await pressKey("y", { altKey: true });
  await vi.waitFor(() => expect(setAssetCustomTag).toHaveBeenCalledTimes(1));
});

it("ignores inputs, dialogs, settings, composition and held-key repeats", async () => {
  for (const selector of ["input", "textarea", "select"]) {
    const input = document.createElement(selector);
    host.appendChild(input);
    await pressKey("t", { ctrlKey: true }, input);
  }
  const dialog = document.createElement("div");
  dialog.setAttribute("role", "dialog");
  host.appendChild(dialog);
  await pressKey("t", { ctrlKey: true }, dialog);
  await pressKey("t", { ctrlKey: true, isComposing: true });
  await pressKey("t", { ctrlKey: true, repeat: true });
  useWorkspaceStore.setState({ settingsOpen: true });
  await pressKey("t", { ctrlKey: true });
  expect(getAssetTagAssignmentsByPath).not.toHaveBeenCalled();
  expect(setAssetCustomTag).not.toHaveBeenCalled();
});

it("refreshes assignments and filtered results even if part of a mixed selection fails", async () => {
  tagged.add("/demo/a.arw");
  await act(async () => useWorkspaceStore.setState({ selectedIds: ["a.arw", "b.arw"] }));
  vi.mocked(setAssetCustomTag).mockRejectedValueOnce(new Error("write failed"));
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await pressKey("t", { ctrlKey: true });
  await vi.waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ["asset-tag-assignments"] }));
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["assets"] });
  expect(useWorkspaceStore.getState().lastTagId).toBe(7);
});
