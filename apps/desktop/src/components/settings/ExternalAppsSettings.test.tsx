// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ExternalAppsSettings } from "./ExternalAppsSettings";
import { EXTERNAL_APPS_QUERY_KEY } from "@/lib/media/externalApps";
import { translate } from "@/lib/i18n";
import type { ExternalAppSettings } from "@/types";

const api = vi.hoisted(() => ({ get: vi.fn(), save: vi.fn(), choose: vi.fn() }));
vi.mock("@/lib/api", () => ({ getExternalAppSettings: api.get, updateExternalAppSettings: api.save, chooseExternalApplication: api.choose, isTauri: () => true }));
let root: Root;
let host: HTMLDivElement;
let client: QueryClient;
let saved: ExternalAppSettings;
const settle = async () => { await act(async () => new Promise((resolve) => setTimeout(resolve, 20))); };
const click = async (text: string, last = false) => {
  const matches = [...host.querySelectorAll("button")].filter((button) => button.textContent === text);
  await act(async () => (last ? matches.at(-1)! : matches[0]).click());
  await settle();
};
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.clearAllMocks();
  saved = { apps: [], defaultAppId: null };
  api.get.mockImplementation(async () => saved);
  api.save.mockImplementation(async (value) => { saved = value; return value; });
  api.choose.mockResolvedValue(null);
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  host = document.createElement("div");
  root = createRoot(host);
  await act(async () => root.render(<QueryClientProvider client={client}><ExternalAppsSettings t={(key) => translate("zh-CN", key)} focus={false} /></QueryClientProvider>));
  await settle();
});
afterEach(async () => { await act(async () => root.unmount()); client.clear(); vi.unstubAllGlobals(); });

it("adds applications, preserves default across reordering, and falls back after default removal", async () => {
  api.choose.mockResolvedValueOnce("C:\\编辑软件\\A.exe").mockResolvedValueOnce("C:\\编辑软件\\B.exe");
  await click("＋ 添加软件");
  await click("保存");
  const defaultId = saved.defaultAppId;
  expect(saved.apps[0].name).toBe("A");
  await click("＋ 添加软件");
  await click("保存");
  await click("上移", true);
  expect(saved.apps.map((app) => app.name)).toEqual(["B", "A"]);
  expect(saved.defaultAppId).toBe(defaultId);
  await click("移除", true);
  expect(saved.defaultAppId).toBe(saved.apps[0].id);
  expect(client.getQueryData(EXTERNAL_APPS_QUERY_KEY)).toEqual(saved);
});

it("does not save cancelled program selection and retains draft after a failed save", async () => {
  await click("＋ 添加软件");
  expect(api.save).not.toHaveBeenCalled();
  api.choose.mockResolvedValueOnce("C:\\A.exe");
  api.save.mockRejectedValueOnce(new Error("Application not found"));
  await click("＋ 添加软件");
  await click("保存");
  expect(host.querySelector("[role=alert]")?.textContent).toContain("Application not found");
  expect(host.querySelector("fieldset")).not.toBeNull();
  expect(saved.apps).toEqual([]);
});
