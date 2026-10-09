// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { translate } from "@/lib/i18n";
import { loadPeopleModuleVisible } from "@/lib/browse/workspacePersistence";
import { useWorkspaceStore } from "@/store";
import { SettingsPanel } from "./SettingsPanel";

vi.mock("./ExternalAppsSettings", () => ({
  ExternalAppsSettings: () => <div data-testid="external-apps">external applications</div>,
}));
vi.mock("./RawDecoderPanel", () => ({
  RawDecoderPanel: () => <div data-testid="raw-decoder">RAW decoder</div>,
}));
vi.mock("@/lib/api", () => ({
  checkForUpdates: vi.fn(),
  chooseCacheParent: vi.fn().mockResolvedValue(null),
  clearPreviewCache: vi.fn(),
  getAppInfo: vi.fn().mockResolvedValue({
    name: "OxyViewer",
    version: "0.1.1",
    repositoryUrl: "https://github.com/oxygenkun/oxyviewer",
    author: "oxygenkun",
    license: "AGPL-3.0-only",
  }),
  getCacheSettings: vi.fn().mockResolvedValue({
    location: "C:\\cache",
    defaultLocation: "C:\\cache",
    customParent: null,
    isCustomLocation: false,
    maxSizeBytes: 10 * 1024 ** 3,
    usedSizeBytes: 2 * 1024 ** 3,
  }),
  getHeifCapabilities: vi.fn().mockResolvedValue([]),
  openAboutLink: vi.fn().mockResolvedValue(undefined),
  updateCacheSettings: vi.fn(),
}));

let root: Root;
let host: HTMLDivElement;
let client: QueryClient;

const clickTab = async (label: string) => {
  const tab = [...host.querySelectorAll<HTMLButtonElement>("[role=tab]")]
    .find((button) => button.textContent?.includes(label));
  await act(async () => tab!.click());
  // Settings modules load their own queries; give the first resolution a tick.
  await act(async () => new Promise((resolve) => setTimeout(resolve, 20)));
};

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  window.localStorage.clear();
  useWorkspaceStore.setState({ settingsOpen: true, settingsSection: "general", locale: "zh-CN", peopleModuleVisible: loadPeopleModuleVisible(), peopleMode: false });
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
  expect(host.querySelectorAll("[role=tab]")).toHaveLength(6);
  // Tab labels are single-line: the description only belongs to the panel heading.
  expect(host.querySelectorAll("[role=tab] small")).toHaveLength(0);
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

  await clickTab("快捷键");
  expect(host.querySelector("[role=tabpanel]")?.textContent).toContain("键盘快捷键");

  await clickTab("关于");
  expect(useWorkspaceStore.getState().settingsSection).toBe("about");
  expect(host.querySelector("[role=tabpanel]")?.textContent).toContain("0.1.1");
  expect(host.querySelector("[role=tabpanel]")?.textContent).toContain("检查更新");
  // The running version is stated once, with the update check beside it.
  expect((host.querySelector("[role=tabpanel]")?.textContent ?? "").split("0.1.1")).toHaveLength(2);
  expect(host.querySelectorAll(".about-settings__version > button")).toHaveLength(1);
});

it("opens directly on the external applications tab", async () => {
  await act(async () => useWorkspaceStore.getState().openSettings("externalApps"));
  expect(host.querySelector('[role=tab][aria-selected="true"]')?.textContent).toContain("外部应用");
  expect(host.querySelector('[data-testid="external-apps"]')).not.toBeNull();
});

it("shows the experimental people option disabled by default", () => {
  const toggle = [...host.querySelectorAll<HTMLButtonElement>("button")]
    .find((button) => button.textContent === "显示人物模块（实验性）")!;
  expect(toggle.getAttribute("aria-pressed")).toBe("false");
  expect(useWorkspaceStore.getState().peopleMode).toBe(false);
});

it("hides the active people module, persists the choice, and requires explicit re-entry", async () => {
  await act(async () => {
    useWorkspaceStore.getState().setPeopleModuleVisible(true);
    useWorkspaceStore.getState().setPeopleMode(true);
  });
  const toggle = [...host.querySelectorAll<HTMLButtonElement>("button")]
    .find((button) => button.textContent === "显示人物模块（实验性）")!;
  expect(toggle.getAttribute("aria-pressed")).toBe("true");

  await act(async () => toggle.click());
  expect(toggle.getAttribute("aria-pressed")).toBe("false");
  expect(useWorkspaceStore.getState()).toMatchObject({ peopleModuleVisible: false, peopleMode: false });
  expect(loadPeopleModuleVisible()).toBe(false);
  // A delayed navigation must not reopen a module that was hidden meanwhile.
  await act(async () => useWorkspaceStore.getState().setPeopleMode(true));
  expect(useWorkspaceStore.getState().peopleMode).toBe(false);

  await act(async () => toggle.click());
  expect(loadPeopleModuleVisible()).toBe(true);
  expect(useWorkspaceStore.getState()).toMatchObject({ peopleModuleVisible: true, peopleMode: false });
  await act(async () => useWorkspaceStore.getState().setPeopleMode(true));
  expect(useWorkspaceStore.getState().peopleMode).toBe(true);
});
