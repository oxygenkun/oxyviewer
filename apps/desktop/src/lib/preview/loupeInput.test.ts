// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { LOUPE_ZOOM_SENSITIVITY } from "@/lib/browse/workspacePersistence";
import { listenLoupeInput } from "./loupeInput";

it("scales pinch wheel deltas at the cursor with the latest sensitivity", () => {
  const stage = document.createElement("div");
  vi.spyOn(stage, "getBoundingClientRect").mockReturnValue(new DOMRect(10, 20, 800, 600));
  const zoomBy = vi.fn();
  let sensitivity = LOUPE_ZOOM_SENSITIVITY.defaultValue;
  const panBy = vi.fn();
  const stop = listenLoupeInput(stage, () => sensitivity, zoomBy, panBy);
  const wheel = (deltaMode: number = WheelEvent.DOM_DELTA_PIXEL) => {
    const event = new WheelEvent("wheel", {
      deltaMode, deltaY: -50, ctrlKey: deltaMode === WheelEvent.DOM_DELTA_PIXEL,
      clientX: 420, clientY: 350, cancelable: true,
    });
    stage.dispatchEvent(event);
    return event.defaultPrevented;
  };

  expect(wheel()).toBe(true);
  expect(zoomBy).toHaveBeenLastCalledWith(Math.exp(0.2), { x: 10, y: 30 });
  sensitivity = 50;
  wheel();
  expect(zoomBy).toHaveBeenLastCalledWith(Math.exp(0.05), { x: 10, y: 30 });
  wheel(WheelEvent.DOM_DELTA_LINE);
  expect(zoomBy).toHaveBeenLastCalledWith(Math.exp(0.1), { x: 10, y: 30 });
  stop();
  expect(wheel()).toBe(false);
  expect(zoomBy).toHaveBeenCalledTimes(3);
  expect(panBy).not.toHaveBeenCalled();
});

it("pans in both axes for unmodified pixel scrolling", () => {
  const stage = document.createElement("div");
  const zoomBy = vi.fn();
  const panBy = vi.fn();
  const stop = listenLoupeInput(stage, () => 200, zoomBy, panBy);
  const wheel = new WheelEvent("wheel", { deltaX: 12, deltaY: -23, cancelable: true });
  stage.dispatchEvent(wheel);
  expect(wheel.defaultPrevented).toBe(true);
  expect(panBy).toHaveBeenCalledExactlyOnceWith({ x: -12, y: 23 });
  expect(zoomBy).not.toHaveBeenCalled();
  stop();
});

it("applies incremental pinch scaling and stops when the gesture ends", () => {
  const stage = document.createElement("div");
  const zoomBy = vi.fn();
  const stop = listenLoupeInput(stage, () => 200, zoomBy, vi.fn());
  const gesture = (type: string, scale: number) => {
    const event = Object.assign(new MouseEvent(type, { cancelable: true }), { scale });
    stage.dispatchEvent(event);
    return event.defaultPrevented;
  };

  expect(gesture("gesturestart", 1)).toBe(true);
  expect(gesture("gesturechange", 1.5)).toBe(true);
  expect(gesture("gesturechange", 1.25)).toBe(true);
  expect(gesture("gestureend", 1.25)).toBe(true);
  expect(gesture("gesturechange", 1.5)).toBe(false);
  expect(zoomBy.mock.calls.map(([factor]) => factor)).toEqual([1.5 ** 2, (1.25 / 1.5) ** 2]);
  stop();
});

it("ignores overlay input and wheels already handled by shortcuts", () => {
  const stage = document.createElement("div");
  const overlay = stage.appendChild(document.createElement("div"));
  overlay.className = "loupe__navigator";
  const zoomBy = vi.fn();
  const panBy = vi.fn();
  const stop = listenLoupeInput(stage, () => 200, zoomBy, panBy);
  const wheel = () => new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaY: -50 });
  const overlayWheel = wheel();
  overlay.dispatchEvent(overlayWheel);
  expect(overlayWheel.defaultPrevented).toBe(false);

  const consumedWheel = wheel();
  consumedWheel.preventDefault();
  stage.dispatchEvent(consumedWheel);
  expect(zoomBy).not.toHaveBeenCalled();
  expect(panBy).not.toHaveBeenCalled();
  stop();
});
