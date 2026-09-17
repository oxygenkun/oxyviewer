// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { horizontalWheelDelta, installFilmstripWheel } from "./filmstripWheel";

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("normalizes pixel, line and page wheels and preserves native horizontal direction", () => {
  expect(horizontalWheelDelta({ deltaX: 0, deltaY: -3, deltaMode: 1 }, 18, 800)).toBe(-54);
  expect(horizontalWheelDelta({ deltaX: 0, deltaY: 1, deltaMode: 2 }, 18, 800)).toBe(800);
  expect(horizontalWheelDelta({ deltaX: -40, deltaY: 120, deltaMode: 0 }, 18, 800)).toBe(-40);
});

it("coalesces a frame without losing total wheel distance and cancels on disposal", () => {
  let frame: FrameRequestCallback | undefined;
  const cancel = vi.fn();
  vi.stubGlobal("requestAnimationFrame", vi.fn((callback) => { frame = callback; return 1; }));
  vi.stubGlobal("cancelAnimationFrame", cancel);
  const element = document.createElement("div");
  const dispose = installFilmstripWheel(element);
  const first = new WheelEvent("wheel", { deltaY: 120, cancelable: true });
  element.dispatchEvent(first);
  element.dispatchEvent(new WheelEvent("wheel", { deltaY: -20, cancelable: true }));
  expect(first.defaultPrevented).toBe(true);
  expect(element.scrollLeft).toBe(0);
  expect(requestAnimationFrame).toHaveBeenCalledTimes(1);
  frame?.(0);
  expect(element.scrollLeft).toBe(100);
  element.dispatchEvent(new WheelEvent("wheel", { deltaX: 10 }));
  dispose();
  expect(cancel).toHaveBeenCalledWith(1);
});
