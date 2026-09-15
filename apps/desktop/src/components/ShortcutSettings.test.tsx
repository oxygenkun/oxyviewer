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

const plain = (key: string) => ({ key, ctrl: false, alt: false, shift: false, meta: false });

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

const row = (actionLabel: string) =>
  [...host.querySelectorAll<HTMLElement>(".shortcut-settings__row")]
    .find((candidate) => candidate.querySelector(".shortcut-settings__action")?.textContent === actionLabel)!;

const slotButton = (actionLabel: string, slot: 0 | 1) =>
  row(actionLabel).querySelectorAll<HTMLButtonElement>(".shortcut-settings__keys")[slot];

const slotText = (actionLabel: string, slot: 0 | 1) => slotButton(actionLabel, slot).textContent;

const restoreButton = (actionLabel: string) =>
  row(actionLabel).querySelector<HTMLButtonElement>(".shortcut-settings__actions button")!;

const clickSlot = async (actionLabel: string, slot: 0 | 1) => {
  await act(async () => slotButton(actionLabel, slot).click());
};

it("lists the default bindings grouped by view", () => {
  const text = host.textContent;
  expect(text).toContain("Loupe 视图");
  expect(text).toContain("网格与列表视图");
  expect(text).toContain("上一张");
  expect(text).toContain("选区下移");
  expect(text).toContain("设为 5 星");
  expect(text).toContain("切换蓝色标签");
  expect(slotText("上一张", 0)).toBe("←");
  expect(slotText("切换对焦区域", 0)).toBe("f");
  expect(host.textContent).not.toContain("重新绑定");
});

it("shows both customizable slots per action", () => {
  expect(slotText("清除评分", 0)).toBe("0");
  expect(slotText("清除评分", 1)).toBe("`");
  expect(slotText("上一张", 1)).toBe("未设置");
});

it("rebinds a slot by clicking the key itself", async () => {
  await clickSlot("上一张", 1);
  expect(slotText("上一张", 1)).toBe("按下新快捷键…");

  await pressKey("k");
  expect(useWorkspaceStore.getState().shortcuts["loupe.previousAsset"]).toEqual([plain("arrowleft"), plain("k")]);
  expect(host.querySelector(".shortcut-settings__keys.is-listening")).toBeNull();
});

it("rebinds the first slot without touching the second", async () => {
  await clickSlot("清除评分", 0);
  await pressKey("j");
  expect(useWorkspaceStore.getState().shortcuts["marking.clearRating"]).toEqual([plain("j"), plain("`")]);
});

it("clears a slot with Backspace", async () => {
  await clickSlot("清除评分", 1);
  await pressKey("Backspace");
  expect(useWorkspaceStore.getState().shortcuts["marking.clearRating"]).toEqual([plain("0"), null]);
  expect(slotText("清除评分", 1)).toBe("未设置");
  expect(host.querySelector("[role=alert]")).toBeNull();
});

it("cancels capture with Escape", async () => {
  await clickSlot("上一张", 0);
  await pressKey("Escape");
  expect(useWorkspaceStore.getState().shortcuts["loupe.previousAsset"]).toEqual(DEFAULT_SHORTCUTS["loupe.previousAsset"]);
  expect(host.querySelector(".shortcut-settings__keys.is-listening")).toBeNull();
});

it("rejects a binding that conflicts with another action", async () => {
  await clickSlot("上一张", 0);
  await pressKey("ArrowRight");
  expect(useWorkspaceStore.getState().shortcuts["loupe.previousAsset"]).toEqual(DEFAULT_SHORTCUTS["loupe.previousAsset"]);
  expect(host.querySelector("[role=alert]")?.textContent).toContain("下一张");
});

it("rejects using one key for both slots of an action", async () => {
  await clickSlot("清除评分", 0);
  await pressKey("`");
  expect(useWorkspaceStore.getState().shortcuts["marking.clearRating"]).toEqual(DEFAULT_SHORTCUTS["marking.clearRating"]);
  expect(host.querySelector("[role=alert]")?.textContent).toContain("两个键位");
});

it("allows grid and loupe actions to share a binding", async () => {
  await clickSlot("选区左移", 0);
  await pressKey("ArrowLeft");
  expect(useWorkspaceStore.getState().shortcuts["grid.moveLeft"][0]?.key).toBe("arrowleft");
  expect(host.querySelector("[role=alert]")).toBeNull();
});

it("restores one action to its defaults", async () => {
  await act(async () => useWorkspaceStore.getState().setShortcut("marking.clearRating", 0, plain("j")));
  await act(async () => useWorkspaceStore.getState().setShortcut("marking.clearRating", 1, null));
  expect(restoreButton("清除评分").disabled).toBe(false);
  await act(async () => restoreButton("清除评分").click());
  expect(useWorkspaceStore.getState().shortcuts["marking.clearRating"]).toEqual([plain("0"), plain("`")]);
  expect(restoreButton("清除评分").disabled).toBe(true);
});

it("resets all bindings to defaults", async () => {
  await act(async () => useWorkspaceStore.getState().setShortcut("loupe.nextAsset", 1, plain("j")));
  const resetAll = [...host.querySelectorAll<HTMLButtonElement>("button")]
    .find((button) => button.textContent?.includes("全部恢复默认"))!;
  await act(async () => resetAll.click());
  expect(useWorkspaceStore.getState().shortcuts).toEqual(DEFAULT_SHORTCUTS);
});
