// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { translate } from "../lib/i18n";
import { DEFAULT_SHORTCUTS } from "../lib/shortcuts";
import { useWorkspaceStore } from "../store";
import { ShortcutSettings } from "./ShortcutSettings";

let root: Root;
let host: HTMLDivElement;

const t = (key: Parameters<typeof translate>[1]) => translate("zh-CN", key);

const pressKey = async (key: string, init: KeyboardEventInit = {}) => {
  await act(async () => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));
  });
};

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  window.localStorage.clear();
  useWorkspaceStore.getState().resetShortcuts();
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root.render(<ShortcutSettings t={t} />));
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  vi.unstubAllGlobals();
});

const rebindButton = (actionLabel: string) =>
  [...host.querySelectorAll<HTMLButtonElement>(".shortcut-settings__row")]
    .find((row) => row.textContent?.includes(actionLabel))
    ?.querySelector<HTMLButtonElement>(".shortcut-settings__actions button");

it("lists the default bindings grouped by view", () => {
  const text = host.textContent;
  expect(text).toContain("Loupe 视图");
  expect(text).toContain("网格视图");
  expect(text).toContain("上一张");
  expect(text).toContain("选区下移");
  expect(text).toContain("设为 5 星");
  expect(text).toContain("切换蓝色标签");
  expect(host.querySelector(".shortcut-settings__keys")?.textContent).toBe("←");
});

it("shows built-in alias bindings next to the customizable one", () => {
  const clearRow = [...host.querySelectorAll<HTMLElement>(".shortcut-settings__row")]
    .find((row) => row.textContent?.includes("清除评分"))!;
  const keys = [...clearRow.querySelectorAll(".shortcut-settings__keys")].map((kbd) => kbd.textContent);
  expect(keys).toEqual(["0", "`"]);
});

it("rebinds an action by capturing the next key press", async () => {
  await act(async () => rebindButton("上一张")!.click());
  expect(host.querySelector(".shortcut-settings__keys.is-listening")).not.toBeNull();

  await pressKey("k");
  expect(useWorkspaceStore.getState().shortcuts["loupe.previousAsset"].key).toBe("k");
  expect(host.querySelector(".shortcut-settings__keys.is-listening")).toBeNull();
});

it("cancels capture with Escape", async () => {
  await act(async () => rebindButton("上一张")!.click());
  await pressKey("Escape");
  expect(useWorkspaceStore.getState().shortcuts["loupe.previousAsset"]).toEqual(DEFAULT_SHORTCUTS["loupe.previousAsset"]);
  expect(host.querySelector(".shortcut-settings__keys.is-listening")).toBeNull();
});

it("rejects a binding that conflicts with another action", async () => {
  await act(async () => rebindButton("上一张")!.click());
  await pressKey("ArrowRight");
  expect(useWorkspaceStore.getState().shortcuts["loupe.previousAsset"]).toEqual(DEFAULT_SHORTCUTS["loupe.previousAsset"]);
  expect(host.querySelector("[role=alert]")?.textContent).toContain("下一张");
});

it("allows grid and loupe actions to share a binding", async () => {
  await act(async () => rebindButton("选区左移")!.click());
  await pressKey("ArrowLeft");
  expect(useWorkspaceStore.getState().shortcuts["grid.moveLeft"].key).toBe("arrowleft");
  expect(host.querySelector("[role=alert]")).toBeNull();
});

it("resets all bindings to defaults", async () => {
  await act(async () => useWorkspaceStore.getState().setShortcut("loupe.nextAsset", { key: "j", ctrl: false, alt: false, shift: false, meta: false }));
  const resetAll = [...host.querySelectorAll<HTMLButtonElement>("button")]
    .find((button) => button.textContent?.includes("全部恢复默认"))!;
  await act(async () => resetAll.click());
  expect(useWorkspaceStore.getState().shortcuts).toEqual(DEFAULT_SHORTCUTS);
});
