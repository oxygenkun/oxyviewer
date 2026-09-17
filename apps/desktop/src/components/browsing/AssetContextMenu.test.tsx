// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { AssetContextMenu } from "./AssetContextMenu";
import { translate } from "@/lib/i18n";
import type { AssetSummary, ExternalAppSettings } from "@/types";

let root: Root;
const open = vi.fn();
const dismiss = vi.fn();
const configure = vi.fn();
const asset = { id: "raw", name: "照片 A.ARW", path: "C:\\原图\\照片 A.ARW" } as AssetSummary;
const settings: ExternalAppSettings = { apps: [
  { id: "a", name: "Editor A", executablePath: "C:\\A.exe" },
  { id: "b", name: "Editor B", executablePath: "C:\\B.exe" },
], defaultAppId: "b" };
const buttons = () => [...document.querySelectorAll<HTMLButtonElement>("[role=menuitem]")];
const render = async (value = settings) => {
  await act(async () => root.render(<AssetContextMenu target={{ asset, x: 1000, y: 760 }} settings={value}
    deletionMode="trash" t={(key) => translate("zh-CN", key)} onDismiss={dismiss} onOpen={open}
    onSettings={configure} onCopy={vi.fn()} onReveal={vi.fn()} onTrash={vi.fn()} />));
};
const key = async (value: string) => { await act(async () => document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true }))); };
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.clearAllMocks();
  root = createRoot(document.createElement("div"));
});
afterEach(async () => { await act(async () => root.unmount()); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("uses the configured default and sends the clicked original path, independent of selection", async () => {
  await render();
  expect(document.activeElement?.textContent).toBe("通过 Editor B 打开");
  await act(async () => buttons()[0].click());
  expect(open).toHaveBeenCalledWith(asset.path, "b");
  expect(dismiss).toHaveBeenCalledOnce();
});

it("keeps a hovered submenu open when its trigger is clicked", async () => {
  await render();
  const trigger = buttons()[1];
  await act(async () => trigger.dispatchEvent(new MouseEvent("pointerover", { bubbles: true })));
  expect(document.querySelectorAll("[role=menu]")).toHaveLength(2);
  await act(async () => trigger.click());
  expect(document.querySelectorAll("[role=menu]")).toHaveLength(2);
  expect(document.activeElement?.textContent).toBe("通过 Editor A 打开");
});

it("supports keyboard submenu navigation, ordered applications, More and Escape", async () => {
  await render();
  await key("ArrowDown");
  await key("ArrowRight");
  expect(document.activeElement?.textContent).toBe("通过 Editor A 打开");
  await key("ArrowDown");
  expect(document.activeElement?.textContent).toBe("通过 Editor B 打开");
  await key("End");
  expect(document.activeElement?.textContent).toBe("更多…");
  await act(async () => (document.activeElement as HTMLButtonElement).click());
  expect(open).toHaveBeenCalledWith(asset.path);
  dismiss.mockClear();
  await key("Escape");
  expect(document.querySelectorAll("[role=menu]")).toHaveLength(1);
  expect(document.activeElement?.textContent).toBe("通过…打开");
  expect(dismiss).not.toHaveBeenCalled();
  await key("Escape");
  expect(dismiss).toHaveBeenCalledOnce();
});

it("offers configuration without a default and keeps menus inside the viewport", async () => {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const left = parseFloat(this.style.left) || 0;
    const top = parseFloat(this.style.top) || 0;
    return { left, top, width: 216, height: 200, right: left + 216, bottom: top + 200, x: left, y: top, toJSON: () => ({}) };
  });
  await render({ apps: [], defaultAppId: null });
  expect(buttons()[0].textContent).toBe("通过…打开");
  await key("ArrowRight");
  for (const menu of document.querySelectorAll<HTMLElement>("[role=menu]")) {
    expect(parseFloat(menu.style.left) + 216).toBeLessThanOrEqual(window.innerWidth - 8);
    expect(parseFloat(menu.style.top) + 200).toBeLessThanOrEqual(window.innerHeight - 8);
  }
  await key("End");
  expect(document.activeElement?.textContent).toBe("设置打开软件…");
  await act(async () => (document.activeElement as HTMLButtonElement).click());
  expect(configure).toHaveBeenCalledOnce();
});
