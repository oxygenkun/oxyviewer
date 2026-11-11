// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Select } from "./Select";

let host: HTMLDivElement;
let root: Root;
const change = vi.fn();
function Harness({ disabled = false }: { disabled?: boolean }) {
  const [value, setValue] = useState("");
  return <Select label="Sort tags" value={value} disabled={disabled}
    options={[{ value: "", label: "Tree" }, { value: "recent", label: "Recent", disabled: true },
      { value: "used", label: "Most used" }]}
    onValueChange={next => { change(next); setValue(next); }} />;
}
async function key(value: string) {
  await act(async () => document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", {
    key: value, bubbles: true, cancelable: true,
  })));
  await act(async () => new Promise(resolve => setTimeout(resolve, 15)));
}
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  change.mockClear(); host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });

it("selects by keyboard, skips disabled options, and returns focus from the portal", async () => {
  await act(async () => root.render(<Harness />));
  const trigger = host.querySelector("button")!; trigger.focus();
  await key("ArrowDown");
  expect(host.querySelector('[role="menu"]')).toBeNull();
  expect(document.querySelector('[role="menuitemradio"][aria-checked="true"]')?.textContent).toBe("Tree");
  expect(document.activeElement?.textContent).toBe("Tree");
  await key("ArrowDown");
  expect(document.activeElement?.textContent).toBe("Most used");
  await key("Enter");
  expect(change).toHaveBeenCalledExactlyOnceWith("used");
  expect(trigger.textContent).toBe("Most used");
  expect(document.activeElement).toBe(trigger);
  expect(document.querySelector('[role="menu"]')).toBeNull();
});

it("isolates menu keys and dismisses with Escape without changing the selection", async () => {
  await act(async () => root.render(<Harness />));
  const trigger = host.querySelector("button")!; trigger.focus(); await key("ArrowDown");
  const shortcut = vi.fn(); document.addEventListener("keydown", shortcut);
  try {
    await key("End"); await key("Escape");
    expect(shortcut).not.toHaveBeenCalled();
    expect(change).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(trigger);
    expect(trigger.textContent).toBe("Tree");
  } finally { document.removeEventListener("keydown", shortcut); }
});

it("prevents opening a disabled selector", async () => {
  await act(async () => root.render(<Harness disabled />));
  expect(host.querySelector("button")!.disabled).toBe(true);
  await act(async () => host.querySelector("button")!.click());
  expect(document.querySelector('[role="menu"]')).toBeNull();
  expect(change).not.toHaveBeenCalled();
});
