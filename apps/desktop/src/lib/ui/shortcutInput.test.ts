// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { listenShortcutInput } from "./shortcutInput";
import { shortcutFromEvent } from "./shortcuts";

const cleanups: Array<() => void> = [];
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  window.dispatchEvent(new Event("blur"));
  document.body.innerHTML = "";
});

it("runs a bound wheel before target zoom handlers, leaving unbound scrolling alone", () => {
  const target = document.createElement("div");
  document.body.append(target);
  const zoom = vi.fn();
  target.addEventListener("wheel", zoom);
  const action = vi.fn();
  cleanups.push(listenShortcutInput((event) => {
    if (shortcutFromEvent(event)?.key !== "wheel_left") return;
    event.preventDefault();
    action();
  }));
  const wheel = new WheelEvent("wheel", { deltaX: -100, bubbles: true, cancelable: true });
  target.dispatchEvent(wheel);
  expect(action).toHaveBeenCalledOnce();
  expect(wheel.defaultPrevented).toBe(true);
  expect(zoom).not.toHaveBeenCalled();
  const unbound = new WheelEvent("wheel", { deltaY: 100, bubbles: true, cancelable: true });
  target.dispatchEvent(unbound);
  expect(unbound.defaultPrevented).toBe(false);
  expect(zoom).toHaveBeenCalledOnce();
});

it("consumes mouse release/auxclick across view changes without firing a second action", () => {
  const action = vi.fn();
  const stop = listenShortcutInput((event) => {
    if (shortcutFromEvent(event)?.key !== "mbtn_back") return;
    event.preventDefault();
    action();
  });
  const down = new MouseEvent("mousedown", { button: 3, cancelable: true });
  window.dispatchEvent(down);
  expect(down.defaultPrevented).toBe(true);
  stop();
  cleanups.push(listenShortcutInput(action));
  for (const type of ["mouseup", "auxclick"]) {
    const event = new MouseEvent(type, { button: 3, cancelable: true });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  }
  expect(action).toHaveBeenCalledOnce();
  const unbound = new MouseEvent("mousedown", { button: 4, cancelable: true });
  window.dispatchEvent(unbound);
  expect(unbound.defaultPrevented).toBe(false);
});

it("gives shortcut recording priority and removes listeners on cleanup", () => {
  const action = vi.fn();
  cleanups.push(listenShortcutInput(action));
  const capture = vi.fn((event: KeyboardEvent | MouseEvent) => event.preventDefault());
  const stopCapture = listenShortcutInput(capture, { capture: true });
  cleanups.push(stopCapture);
  window.dispatchEvent(new MouseEvent("mousedown", { button: 1, cancelable: true }));
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", cancelable: true }));
  expect(capture).toHaveBeenCalledTimes(2);
  expect(action).not.toHaveBeenCalled();
  stopCapture();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", cancelable: true }));
  expect(action).toHaveBeenCalledOnce();
});
