// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { FolderNameButton } from "./FolderNameButton";

let root: Root;
let container: HTMLDivElement;
const navigate = vi.fn();
const path = "/Photos/20260913 完整文件夹名称";

function label() {
  return container.querySelector("span")!;
}

// jsdom performs no layout, so the clipped/fitting geometry is stubbed per case.
function setGeometry(clientWidth: number, scrollWidth: number) {
  Object.defineProperty(label(), "clientWidth", { configurable: true, value: clientWidth });
  Object.defineProperty(label(), "scrollWidth", { configurable: true, value: scrollWidth });
}

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(
    <FolderNameButton title={path} onClick={navigate}>
      <svg />
      <span>完整文件夹名称</span>
    </FolderNameButton>,
  ));
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

it("stays silent while the name fits inside the row", async () => {
  setGeometry(200, 200);
  const button = container.querySelector("button")!;
  expect(button.hasAttribute("title")).toBe(false);
  await act(async () => button.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })));
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
  expect(button.getAttribute("aria-describedby")).toBeNull();
});

it("shows the full path immediately without a native tooltip and dismisses on leave", async () => {
  setGeometry(80, 200);
  const button = container.querySelector("button")!;
  expect(button.hasAttribute("title")).toBe(false);
  await act(async () => button.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })));
  const tooltip = document.querySelector('[role="tooltip"]')!;
  expect(tooltip.textContent).toBe(path);
  expect(button.getAttribute("aria-describedby")).toBe(tooltip.id);
  await act(async () => button.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body })));
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
});

it("dismisses on Escape, scrolling, and pointer down while preserving navigation", async () => {
  setGeometry(80, 200);
  const button = container.querySelector("button")!;
  for (const event of [
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    new Event("scroll", { bubbles: true }),
    new MouseEvent("pointerdown", { bubbles: true }),
  ]) {
    await act(async () => button.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })));
    expect(document.querySelector('[role="tooltip"]')).not.toBeNull();
    await act(async () => button.dispatchEvent(event));
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
  }
  await act(async () => button.click());
  expect(navigate).toHaveBeenCalledOnce();
});
