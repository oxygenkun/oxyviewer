// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { patchMetadata } from "@/lib/api";
import { useMetadataProjectionStore } from "@/lib/projection/metadataProjection";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary } from "@/types";
import { useMarkingShortcuts } from "./useMarkingShortcuts";

vi.mock("@/lib/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/lib/api")>();
  return { ...original, patchMetadata: vi.fn().mockResolvedValue("job-1") };
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

function Probe({ suppressed = false }: { suppressed?: boolean }) {
  useMarkingShortcuts(assets, suppressed);
  return null;
}

let root: Root;
let host: HTMLDivElement;
let client: QueryClient;

const pressKey = async (key: string) => {
  await act(async () => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  });
};

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.mocked(patchMetadata).mockClear();
  window.localStorage.clear();
  useWorkspaceStore.getState().resetShortcuts();
  useWorkspaceStore.setState({ selectedIds: ["a.arw"], activeId: "a.arw", settingsOpen: false });
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
  expect(patchMetadata).not.toHaveBeenCalled();
});
