// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { FolderSettingsMenu } from "./FolderSettingsMenu";

let host: HTMLDivElement;
let root: Root;
const sort = vi.fn();
const drag = vi.fn();
function Harness({ disabled = false }: { disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  return <FolderSettingsMenu open={open} onOpenChange={setOpen} disabled={disabled}
    folderSort="nameAscending" onFolderSortChange={sort} folderDragEnabled
    onFolderDragEnabledChange={drag} t={key => key} />;
}
async function key(value: string) {
  await act(async () => document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true, cancelable: true })));
  await act(async () => new Promise(resolve => setTimeout(resolve, 15)));
}
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  sort.mockClear(); drag.mockClear();
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });

it("supports keyboard submenu selection and returns focus to its trigger", async () => {
  await act(async () => root.render(<Harness />));
  const trigger = host.querySelector("button")!; trigger.focus();
  await key("ArrowDown");
  expect(document.activeElement?.textContent).toBe("folderSort");
  await key("ArrowRight");
  const options = document.querySelectorAll('[role="menuitemradio"]');
  expect(options).toHaveLength(3);
  expect(options[1].getAttribute("aria-checked")).toBe("true");
  await key("End"); await key("Enter");
  expect(sort).toHaveBeenCalledWith("nameDescending");
  expect(document.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement).toBe(trigger);
});

it("toggles dragging once, closes, and supports Escape without changing settings", async () => {
  await act(async () => root.render(<Harness />));
  const trigger = host.querySelector("button")!; trigger.focus(); await key("ArrowDown");
  await key("End");
  expect(document.activeElement?.getAttribute("aria-checked")).toBe("true");
  await key("Enter"); expect(drag).toHaveBeenCalledExactlyOnceWith(false);
  expect(document.activeElement).toBe(trigger);
  expect(trigger.getAttribute("data-restored-focus")).toBe("true");
  await key("Tab");
  expect(trigger.hasAttribute("data-restored-focus")).toBe(false);
  await key("ArrowDown"); await key("Escape");
  expect(document.querySelector('[role="menu"]')).toBeNull();
  expect(sort).not.toHaveBeenCalled(); expect(drag).toHaveBeenCalledTimes(1);
});

it("disables the trigger without folders", async () => {
  await act(async () => root.render(<Harness disabled />));
  expect(host.querySelector("button")!.disabled).toBe(true);
});
