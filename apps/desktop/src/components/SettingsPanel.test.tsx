// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { translate } from "../lib/i18n";
import { useWorkspaceStore } from "../store";
import { SettingsPanel } from "./SettingsPanel";

vi.mock("./ExternalAppsSettings", () => ({
  ExternalAppsSettings: () => <div data-testid="external-apps">external applications</div>,
}));
vi.mock("./RawDecoderPanel", () => ({
  RawDecoderPanel: () => <div data-testid="raw-decoder">RAW decoder</div>,
}));
vi.mock("../lib/api", () => ({
  chooseCacheParent: vi.fn().mockResolvedValue(null),
  clearPreviewCache: vi.fn(),
  getCacheSettings: vi.fn().mockResolvedValue({
    location: "C:\\cache",
    defaultLocation: "C:\\cache",
    customParent: null,
    isCustomLocation: false,
    maxSizeBytes: 10 * 1024 ** 3,
    usedSizeBytes: 2 * 1024 ** 3,
  }),
  getHeifCapabilities: vi.fn().mockResolvedValue([]),
  updateCacheSettings: vi.fn(),
}));

let root: Root;
let host: HTMLDivElement;
let client: QueryClient;

const clickTab = async (label: string) => {
  const tab = [...host.querySelectorAll<HTMLButtonElement>("[role=tab]")]
    .find((button) => button.textContent?.includes(label));
  await act(async () => tab!.click());
};

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  useWorkspaceStore.setState({ settingsOpen: true, settingsSection: "general", locale: "zh-CN" });
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  host = document.createElement("div");
  root = createRoot(host);
  await act(async () => root.render(
    <QueryClientProvider client={client}>
      <SettingsPanel t={(key) => translate("zh-CN", key)} />
    </QueryClientProvider>,
  ));
});

afterEach(async () => {
  await act(async () => root.unmount());
  client.clear();
  vi.unstubAllGlobals();
});

it("separates settings into tabs and renders only the selected module", async () => {
  expect(host.querySelectorAll("[role=tab]")).toHaveLength(4);
  expect(host.querySelector('[role=tab][aria-selected="true"]')?.textContent).toContain("常规");
  expect(host.querySelector("[role=tabpanel]")?.textContent).toContain("语言");

  await clickTab("显示");
  expect(useWorkspaceStore.getState().settingsSection).toBe("display");
  expect(host.querySelector("[role=tabpanel]")?.textContent).toContain("评分与颜色标签");
  expect(host.querySelector("[role=tabpanel]")?.textContent).not.toContain("UI 文字大小");

  await clickTab("媒体与缓存");
  expect(host.querySelector("[role=tabpanel]")?.textContent).toContain("预览缓存");
  expect(host.querySelector('[data-testid="raw-decoder"]')).not.toBeNull();

  await clickTab("外部应用");
  expect(host.querySelector('[data-testid="external-apps"]')).not.toBeNull();
});

it("opens directly on the external applications tab", async () => {
  await act(async () => useWorkspaceStore.getState().openSettings("externalApps"));
  expect(host.querySelector('[role=tab][aria-selected="true"]')?.textContent).toContain("外部应用");
  expect(host.querySelector('[data-testid="external-apps"]')).not.toBeNull();
});
