// @vitest-environment jsdom
import { act, useRef, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Button, IconButton } from "./Button";
import { Field } from "./Field";
import { Dialog } from "./Dialog";

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove(); vi.unstubAllGlobals();
});

it("blocks repeated activation while loading and names icon-only actions", async () => {
  const click = vi.fn();
  await act(async () => root.render(<><Button loading onClick={click}>Save</Button><IconButton label="Close">×</IconButton></>));
  const button = host.querySelector("button")!;
  await act(async () => button.click());
  expect(click).not.toHaveBeenCalled();
  expect(button.getAttribute("aria-busy")).toBe("true");
  expect(host.querySelector('[aria-label="Close"]')?.getAttribute("type")).toBe("button");
});

it("links field labels and help/error text without losing caller descriptions", async () => {
  await act(async () => root.render(<Field label="Limit" hint="In GB" error="Too large" aria-describedby="external" />));
  const input = host.querySelector("input")!;
  expect(host.querySelector("label")?.htmlFor).toBe(input.id);
  expect(input.getAttribute("aria-invalid")).toBe("true");
  const ids = input.getAttribute("aria-describedby")!.split(" ");
  expect(ids[0]).toBe("external");
  expect(ids.slice(1).map(id => document.getElementById(id)?.textContent)).toEqual(["In GB", "Too large"]);
});

function DialogHarness() {
  const [open, setOpen] = useState(false);
  const cancel = useRef<HTMLButtonElement>(null);
  return <><button onClick={() => setOpen(true)}>Open</button>
    <Dialog open={open} onOpenChange={setOpen} title="Delete" description="Selected file"
      initialFocusRef={cancel} dismissOnOutsideClick={false} role="alertdialog">
      <Button ref={cancel} onClick={() => setOpen(false)}>Cancel</Button><Button>Delete</Button>
    </Dialog></>;
}

it("focuses the safe action, contains Tab, isolates shortcuts and restores focus on Escape", async () => {
  await act(async () => root.render(<DialogHarness />));
  const trigger = host.querySelector("button")!;
  trigger.focus();
  await act(async () => trigger.click());
  const dialog = document.querySelector('[role="alertdialog"]')!;
  expect(document.activeElement?.textContent).toBe("Cancel");
  expect(document.getElementById(dialog.getAttribute("aria-describedby")!)?.textContent).toBe("Selected file");
  const actions = dialog.querySelectorAll("button");
  actions[1].focus();
  await act(async () => actions[1].dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true })));
  expect(document.activeElement).toBe(actions[0]);
  const shortcut = vi.fn(); document.addEventListener("keydown", shortcut);
  await act(async () => actions[0].dispatchEvent(new KeyboardEvent("keydown", { key: "1", bubbles: true })));
  expect(shortcut).not.toHaveBeenCalled(); document.removeEventListener("keydown", shortcut);
  await act(async () => actions[0].dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
  await act(async () => new Promise(resolve => setTimeout(resolve, 10)));
  expect(document.querySelector('[role="alertdialog"]')).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
