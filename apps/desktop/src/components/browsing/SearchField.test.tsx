// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SearchField } from "./SearchField";
import { splitTagInput } from "@/lib/assets/tagSearch";

let host: HTMLDivElement;
let root: Root;
const change = vi.fn();
const remove = vi.fn();
const select = vi.fn();
const input = () => host.querySelector("input")!;
const key = async (value: string, extra: KeyboardEventInit = {}) => {
  await act(async () => input().dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true, cancelable: true, ...extra })));
};
async function type(value: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input(), value);
    input().dispatchEvent(new Event("input", { bubbles: true }));
  });
}
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  await act(async () => root.render(<SearchField value="" onChange={change} label="Search" clearLabel="Clear"
    tokens={[{ id: "1", label: "People › Family", removeLabel: "Remove Family" }, { id: "2", label: "Trips", removeLabel: "Remove Trips" }]}
    onRemoveToken={remove} suggestions={[{ id: "3", label: "Sea" }, { id: "4", label: "Sky" }]} onSelectSuggestion={select} />));
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.clearAllMocks(); vi.unstubAllGlobals(); });
it("keeps hash completion separate from literal filename fragments", () => {
  expect(splitTagInput("coast #人物")).toEqual({ filename: "coast", needle: "人物" });
  expect(splitTagInput("#")).toEqual({ filename: "", needle: "" });
  expect(splitTagInput("IMG#001")).toEqual({ filename: "IMG#001" });
});
it("defers filename commits and suggestion submission during IME composition", async () => {
  await key("ArrowDown");
  await act(async () => input().dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true })));
  await type("zhong"); await key("Enter", { isComposing: true });
  expect(change).not.toHaveBeenCalled(); expect(select).not.toHaveBeenCalled();
  await type("中");
  await act(async () => input().dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "中" })));
  expect(change).toHaveBeenCalledWith("中");
});
it("requires two Backspaces and Escape cancels pending deletion", async () => {
  await key("Backspace"); expect(remove).not.toHaveBeenCalled();
  await key("Escape"); await key("Backspace"); expect(remove).not.toHaveBeenCalled();
  await key("Backspace"); expect(remove).toHaveBeenCalledWith("2");
});
it("connects combobox options, selects by keyboard and closes without clearing", async () => {
  await key("ArrowDown"); await key("ArrowDown");
  const active = document.getElementById(input().getAttribute("aria-activedescendant")!);
  expect(active?.textContent).toBe("Sky");
  expect(input().getAttribute("aria-controls")).toBe(host.querySelector('[role="listbox"]')?.id);
  await key("Enter"); expect(select).toHaveBeenCalledWith("4"); expect(change).not.toHaveBeenCalled();
  await key("Escape"); expect(input().getAttribute("aria-expanded")).toBe("false");
  expect(document.activeElement).toBe(input());
});
it("shows collapsed tags and uses the same selection callback for a click", async () => {
  const more = [...host.querySelectorAll("button")].find((button) => button.textContent === "+1")!;
  await act(async () => more.click());
  expect(host.textContent).toContain("People › Family"); expect(host.textContent).toContain("Trips");
  await act(async () => (host.querySelector('[role="option"]') as HTMLElement).click());
  expect(select).toHaveBeenCalledWith("3");
});
it("focuses with Ctrl+K but leaves other text editors alone", async () => {
  await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true })));
  expect(document.activeElement).toBe(input());
  const editor = document.createElement("textarea"); host.append(editor); await act(async () => editor.focus());
  await act(async () => editor.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true })));
  expect(document.activeElement).toBe(editor); expect(change).not.toHaveBeenCalled();
});
