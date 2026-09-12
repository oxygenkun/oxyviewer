// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { RawDecoderPanel } from "./RawDecoderPanel";
import type { AssetSummary, RawDecoderStatus } from "../types";
import { assetRenderQueryKey } from "../lib/preview";

const api = vi.hoisted(() => ({ status: vi.fn(), install: vi.fn(), retry: vi.fn(), invalidate: vi.fn() }));
vi.mock("../lib/api", () => ({ getRawDecoderStatus: api.status, openRawDecoderInstallPage: api.install,
  retryRawFull: api.retry }));
vi.mock("../lib/imageProjection", () => ({ invalidateImageProjection: api.invalidate }));
const asset = { id: "raw", path: "C:\\photos\\a.arw", kind: "raw" } as AssetSummary;
const missing: RawDecoderStatus = { installAvailable: true, availability: "missing", codecs: [], detail: null,
  attempt: { state: "unsupportedFile", detail: "codec not found" } };
let value: RawDecoderStatus;
let host: HTMLDivElement;
let root: Root;
let client: QueryClient;
const settle = async () => { await act(async () => new Promise((resolve) => setTimeout(resolve, 20))); };
const render = async (compact = false, failed = false) => {
  await act(async () => root.render(<QueryClientProvider client={client}>
    <RawDecoderPanel asset={asset} compact={compact} failed={failed} t={(key) => key} />
  </QueryClientProvider>));
  await settle();
};
const click = async (label: string) => {
  const button = [...host.querySelectorAll("button")].find((button) => button.textContent === label);
  expect(button).toBeDefined();
  await act(async () => button!.click());
  await settle();
};
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  Object.values(api).forEach((mock) => mock.mockReset());
  value = { ...missing };
  api.status.mockImplementation(async () => value);
  api.install.mockResolvedValue("store");
  api.retry.mockResolvedValue(undefined);
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  host = document.createElement("div");
  root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); client.clear(); vi.unstubAllGlobals(); });

it("offers optional installation, checks on return, and explicitly retries the current file", async () => {
  await render();
  expect(host.textContent).toContain("rawBundledHint");
  expect(host.textContent).toContain("rawSystemMissing");
  expect(api.install).not.toHaveBeenCalled();
  await click("rawOpenStore");
  expect(api.install).toHaveBeenCalledWith(false, expect.anything());
  value = { ...missing, availability: "available", installAvailable: false, attempt: null };
  await act(async () => window.dispatchEvent(new Event("focus")));
  await settle();
  expect(api.status).toHaveBeenCalledWith(asset.path, true);
  expect(host.textContent).toContain("rawSystemAvailable");
  expect(host.querySelector("select")).toBeNull();
  const fullKey = assetRenderQueryKey(asset, { type: "generatedImage", requestLevel: "full" });
  const thumbnailKey = assetRenderQueryKey(asset, { type: "generatedImage", requestLevel: "thumbnail" });
  const otherFullKey = assetRenderQueryKey({ ...asset, id: "other", path: "C:\\photos\\b.arw" },
    { type: "generatedImage", requestLevel: "full" });
  for (const queryKey of [fullKey, thumbnailKey, otherFullKey]) client.setQueryData(queryKey, "retained");
  await click("rawRetryFull");
  expect(api.retry).toHaveBeenCalledWith(asset.path);
  expect(api.invalidate).toHaveBeenCalledWith(asset.path, "full");
  expect(client.getQueryState(fullKey)?.isInvalidated).toBe(true);
  expect(client.getQueryState(thumbnailKey)?.isInvalidated).toBe(false);
  expect(client.getQueryState(otherFullKey)?.isInvalidated).toBe(false);
});

it("keeps installed-but-unsupported file failures separate from installation", async () => {
  value = { ...missing, availability: "available" };
  await render(true);
  expect(host.textContent).toContain("rawFileFallback");
  expect(host.textContent).not.toContain("rawOpenStore");
  await click("rawDismissHint");
  expect(host.textContent).toBe("");
  await render(true, true);
  expect(host.textContent).toContain("rawFullFailed");
  expect(host.textContent).toContain("rawRetryFull");
});

it("supports web fallback and restart guidance without automatically installing anything", async () => {
  await render();
  await click("rawOfficialPage");
  expect(api.install).toHaveBeenCalledWith(true, expect.anything());
  await click("rawCheckAgain");
  expect(host.textContent).toContain("rawRestartHint");
  api.retry.mockRejectedValueOnce(new Error("disk unavailable"));
  await click("rawRetryFull");
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("disk unavailable");
});
