// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { translate } from "@/lib/i18n";
import { checkForUpdates, getAppInfo, openAboutLink } from "@/lib/api";
import { AboutSettings, UP_TO_DATE_DISMISS_MS } from "./AboutSettings";

vi.mock("@/lib/api", () => ({
  checkForUpdates: vi.fn(),
  getAppInfo: vi.fn(),
  openAboutLink: vi.fn(),
}));

const appInfo = {
  name: "OxyViewer",
  version: "0.1.1",
  repositoryUrl: "https://github.com/oxygenkun/oxyviewer",
  author: "oxygenkun",
  license: "AGPL-3.0-only OR LicenseRef-OxyViewer-Commercial",
};

let root: Root;
let host: HTMLDivElement;
let client: QueryClient;

const settle = async () =>
  act(async () => new Promise((resolve) => setTimeout(resolve, 20)));

const render = async () => {
  root = createRoot(host);
  await act(async () =>
    root.render(
      <QueryClientProvider client={client}>
        <AboutSettings t={(key) => translate("zh-CN", key)} />
      </QueryClientProvider>,
    ),
  );
  await settle();
};

const clickButton = (label: string) => {
  const button = [...host.querySelectorAll("button")].find((candidate) =>
    candidate.textContent?.includes(label),
  );
  if (!button) throw new Error(`missing button: ${label}`);
  return button;
};

const click = async (label: string) => {
  const button = clickButton(label);
  await act(async () => button.click());
  await settle();
};

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div");
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.mocked(getAppInfo).mockResolvedValue(appInfo);
  vi.mocked(checkForUpdates).mockReset();
  vi.mocked(openAboutLink).mockReset();
  vi.mocked(openAboutLink).mockResolvedValue(undefined);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  client.clear();
  vi.unstubAllGlobals();
});

it("reports the running build and opens the fixed project links", async () => {
  await render();

  expect(host.textContent).toContain("0.1.1");
  expect(host.textContent).toContain("github.com/oxygenkun/oxyviewer");
  expect(host.textContent).toContain("oxygenkun");
  // Version is stated once, inline with the update check, not repeated in the facts.
  expect((host.textContent ?? "").split("0.1.1")).toHaveLength(2);
  expect(host.querySelector(".about-settings__version > button")?.textContent).toContain("检查更新");
  expect([...host.querySelectorAll(".about-settings__facts dt")].map((term) => term.textContent)).toEqual([
    "仓库",
    "作者",
    "许可证",
  ]);
  // The license is a short link carrying the full SPDX id as its title.
  expect(
    [...host.querySelectorAll(".about-settings__facts dd button")].at(-1)?.getAttribute("title"),
  ).toBe(appInfo.license);
  // No network disclaimer: the check is explicit and the button says so.
  expect(host.textContent).not.toContain("后台联网");

  await click("github.com/oxygenkun/oxyviewer");
  expect(openAboutLink).toHaveBeenCalledWith("repository");

  await click("AGPL");
  expect(openAboutLink).toHaveBeenCalledWith("license");
});

it("reports a newer release and offers its release page", async () => {
  vi.mocked(checkForUpdates).mockResolvedValue({
    currentVersion: "0.1.1",
    latestVersion: "0.2.0",
    updateAvailable: true,
    releaseUrl: "https://github.com/oxygenkun/oxyviewer/releases/tag/v0.2.0",
    releaseName: "OxyViewer v0.2.0",
    publishedAt: "2026-01-02T03:04:05Z",
  });
  await render();

  expect(host.textContent).not.toContain("发现新版本");
  await click("检查更新");

  expect(checkForUpdates).toHaveBeenCalledTimes(1);
  expect(host.textContent).toContain("发现新版本 0.2.0");
  expect(host.textContent).toContain("OxyViewer v0.2.0 · 2026-01-02");

  await click("打开发布页");
  expect(openAboutLink).toHaveBeenCalledWith(
    "releases",
    "https://github.com/oxygenkun/oxyviewer/releases/tag/v0.2.0",
  );
});

it("confirms the current version beside the button when no newer release exists", async () => {
  vi.mocked(checkForUpdates).mockResolvedValue({
    currentVersion: "0.1.1",
    latestVersion: "0.1.1",
    updateAvailable: false,
    releaseUrl: "https://github.com/oxygenkun/oxyviewer/releases/tag/v0.1.1",
  });
  await render();

  await click("检查更新");

  expect(host.querySelector(".about-settings__version .about-settings__result")?.textContent).toBe(
    "已是最新版本",
  );
  // The running version stays stated once, in the row above the result.
  expect((host.textContent ?? "").split("0.1.1")).toHaveLength(2);
  expect(host.textContent).not.toContain("打开发布页");
});

it("retires the up-to-date confirmation on its own", async () => {
  vi.mocked(checkForUpdates).mockResolvedValue({
    currentVersion: "0.1.1",
    latestVersion: "0.1.1",
    updateAvailable: false,
    releaseUrl: "https://github.com/oxygenkun/oxyviewer/releases/tag/v0.1.1",
  });
  vi.useFakeTimers();
  try {
    root = createRoot(host);
    await act(async () => {
      root.render(
        <QueryClientProvider client={client}>
          <AboutSettings t={(key) => translate("zh-CN", key)} />
        </QueryClientProvider>,
      );
      await vi.advanceTimersByTimeAsync(20);
    });
    await act(async () => {
      clickButton("检查更新").click();
      await vi.advanceTimersByTimeAsync(20);
    });
    expect(host.querySelector(".about-settings__result")).not.toBeNull();

    await act(async () => void (await vi.advanceTimersByTimeAsync(UP_TO_DATE_DISMISS_MS)));

    expect(host.querySelector(".about-settings__result")).toBeNull();
    expect(host.textContent).toContain("版本 0.1.1");
  } finally {
    vi.useRealTimers();
  }
});

it("surfaces a failed check without leaving the panel stuck", async () => {
  vi.mocked(checkForUpdates).mockRejectedValue("the release check timed out");
  await render();

  await click("检查更新");

  expect(host.querySelector(".settings-panel__error")?.textContent).toBe(
    "the release check timed out",
  );
  expect(host.textContent).toContain("检查更新");
  expect(host.querySelector("button")?.hasAttribute("disabled")).toBe(false);
});
