// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { notifyManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AssetSummary } from "@/types";
import { Inspector } from "./Inspector";

const api = vi.hoisted(() => ({
  chooseAndConfigureExiftool: vi.fn(), getAssetDetails: vi.fn(), getExiftoolStatus: vi.fn(),
  installExiftool: vi.fn(), patchMetadata: vi.fn(), syncMetadataToEmbedded: vi.fn(),
}));
vi.mock("@/lib/api", () => api);
vi.mock("./TagEditor", () => ({ TagEditor: () => null }));
vi.mock("@/components/browsing/AssetBrowser", () => ({ formatBytes: () => "10 B" }));

const photo: AssetSummary = { id: "photo", path: "/photos/a.jpg", name: "a.jpg", extension: "jpg",
  kind: "jpeg", sizeBytes: 10, modifiedAtMs: 1, hasSidecar: true };
let host: HTMLDivElement;
let root: Root;
let client: QueryClient;
function button(text: string) {
  const result = [...host.querySelectorAll("button")].find(node => node.textContent === text);
  expect(result).toBeDefined();
  return result!;
}
async function click(text: string) {
  await act(async () => { button(text).click(); });
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
}
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.resetAllMocks();
  notifyManager.setNotifyFunction(callback => { act(callback); });
  api.getAssetDetails.mockResolvedValue({ asset: photo, metadata: {}, metadataCapability: {},
    embeddedKeywords: [], embeddedHierarchicalKeywords: [], captureMetadata: {} });
  api.getExiftoolStatus.mockResolvedValue({ available: false });
  api.syncMetadataToEmbedded.mockResolvedValue("job");
  client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  await act(async () => root.render(<QueryClientProvider client={client}>
    <Inspector asset={photo} selectedCount={1} selectedPaths={[photo.path]} t={key => key} />
  </QueryClientProvider>));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
});
afterEach(async () => {
  await act(async () => root.unmount()); host.remove(); client.clear();
  notifyManager.setNotifyFunction(callback => { callback(); }); vi.unstubAllGlobals();
});

it("keeps the inspector prompt open on a failed manual check and resumes its write when PATH becomes available", async () => {
  await click("syncEmbeddedMetadata");
  expect(host.querySelector('[role="dialog"]')).not.toBeNull();
  expect(host.textContent).toContain("exiftoolManualInstallHint");
  await click("recheckExiftool");
  expect(host.querySelector('[role="dialog"]')).not.toBeNull();
  expect(host.querySelector('[role="status"]')?.textContent).toBe("exiftoolNotFound");
  expect(api.syncMetadataToEmbedded).not.toHaveBeenCalled();
  api.getExiftoolStatus.mockResolvedValue({ available: true, source: "path" });
  await click("recheckExiftool");
  expect(api.getExiftoolStatus).toHaveBeenCalledTimes(3);
  expect(api.syncMetadataToEmbedded).toHaveBeenCalledExactlyOnceWith([photo.path]);
  expect(host.querySelector('[role="dialog"]')).toBeNull();
  expect(api.installExiftool).not.toHaveBeenCalled();
  expect(api.chooseAndConfigureExiftool).not.toHaveBeenCalled();
});

it("disables setup actions during a fresh probe and allows another check after a probe error", async () => {
  await click("syncEmbeddedMetadata");
  let reject!: (error: Error) => void;
  api.getExiftoolStatus.mockReturnValue(new Promise((_resolve, fail) => { reject = fail; }));
  await click("recheckExiftool");
  expect(host.querySelector('[role="status"]')?.textContent).toBe("checkingExiftool");
  expect([...host.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].every(node => node.disabled)).toBe(true);
  await act(async () => { reject(new Error("probe failed")); });
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
  expect(host.querySelector('[role="dialog"]')?.textContent).toContain("probe failed");
  expect(button("recheckExiftool").disabled).toBe(false);
  expect(api.syncMetadataToEmbedded).not.toHaveBeenCalled();
  api.getExiftoolStatus.mockResolvedValue({ available: true });
  await click("recheckExiftool");
  expect(api.syncMetadataToEmbedded).toHaveBeenCalledExactlyOnceWith([photo.path]);
});
