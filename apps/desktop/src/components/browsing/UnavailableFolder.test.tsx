// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { UnavailableFolder } from "./UnavailableFolder";
import type { RootRelocationPlan } from "@/types";
const api = vi.hoisted(() => ({ chooseFolder: vi.fn(), planRootRelocation: vi.fn() }));
vi.mock("@/lib/api", () => api);
const plan: RootRelocationPlan = { oldRoot: "/old", newRoot: "/new", entries: [
  { oldPath: "/old/a.jpg", newPath: "/new/a.jpg", status: "unverified", oldIdentityRevision: null, newIdentityRevision: null },
] };
let host: HTMLDivElement;
let root: Root;
const retry = vi.fn(), remove = vi.fn(), relocate = vi.fn();
async function key(key: string) {
  await act(async () => document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true })));
  await act(async () => new Promise(resolve => setTimeout(resolve, 15)));
}
async function menu() { host.querySelector("button")!.focus(); await key("ArrowDown"); }
function button(text: string) { return [...document.querySelectorAll("button")].find(el => el.textContent === text)!; }
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.clearAllMocks();
  api.chooseFolder.mockResolvedValue("/new"); api.planRootRelocation.mockResolvedValue(plan);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  await act(async () => root.render(<UnavailableFolder path="/old" onRetry={retry} onRemove={remove} onRelocate={relocate} t={key => key} />));
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
it("requires acknowledgement before linking uncertain annotations", async () => {
  await menu(); await key("Enter");
  expect(document.querySelector('[role="dialog"]')).not.toBeNull();
  expect(button("folderRelocateConfirm").disabled).toBe(true);
  expect(relocate).not.toHaveBeenCalled();
  await act(async () => document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
  await act(async () => button("folderRelocateConfirm").click());
  expect(relocate).toHaveBeenCalledExactlyOnceWith(plan);
});
it("keeps an unavailable folder and reports a failed retry", async () => {
  retry.mockRejectedValueOnce(new Error("offline"));
  await menu(); await key("ArrowDown"); await key("Enter");
  expect(retry).toHaveBeenCalledExactlyOnceWith("/old");
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("offline");
  expect(host.textContent).toContain("folderUnavailable");
  expect(remove).not.toHaveBeenCalled();
});
it("only removes the list entry after explicit confirmation", async () => {
  await act(async () => host.querySelector<HTMLButtonElement>(".tree-row__remove")!.click());
  expect(document.querySelector('[role="dialog"]')?.textContent).toContain("folderRemoveHint");
  expect(remove).not.toHaveBeenCalled();
  await act(async () => button("cancel").click());
  expect(remove).not.toHaveBeenCalled();
  await act(async () => host.querySelector<HTMLButtonElement>(".tree-row__remove")!.click());
  await act(async () => button("folderRemoveEntry").click());
  expect(remove).toHaveBeenCalledExactlyOnceWith("/old");
});

it("offers exactly two link operations and a separate remove button", async () => {
  expect(host.querySelector(".tree-row__remove svg.lucide-x")).not.toBeNull();
  expect(host.querySelector("svg.lucide-link2, svg.lucide-link-2")).not.toBeNull();
  await menu();
  expect([...document.querySelectorAll('[role="menuitem"]')].map(item => item.textContent))
    .toEqual(["folderRelocate", "folderRetry"]);
});
