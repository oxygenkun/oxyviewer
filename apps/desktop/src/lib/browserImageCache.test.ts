import { describe, expect, it } from "vitest";
import {
  browserImageSourceWhenEnabled,
  clearBrowserImageResources,
  isBrowserImageReady,
  markBrowserImageReady,
  setBrowserImageResourceScope,
} from "./browserImageCache";

describe("browser image resource gating", () => {
  it("does not start an uncached image while resource loading is paused", () => {
    expect(browserImageSourceWhenEnabled("asset://moving.jpg", false)).toBeUndefined();
    expect(browserImageSourceWhenEnabled("asset://moving.jpg", true)).toBe("asset://moving.jpg");
  });

  it("keeps an already decoded image paintable while loading is paused", () => {
    markBrowserImageReady("asset://cached.jpg", { width: 320, height: 240 });

    expect(browserImageSourceWhenEnabled("asset://cached.jpg", false)).toBe(
      "asset://cached.jpg",
    );
  });

  it("retains every resource loaded by the page until an explicit update", () => {
    clearBrowserImageResources();
    for (let index = 0; index < 600; index += 1) {
      markBrowserImageReady(`asset://page-${index}.jpg`, { width: 320, height: 240 });
    }

    expect(isBrowserImageReady("asset://page-0.jpg")).toBe(true);
    expect(isBrowserImageReady("asset://page-599.jpg")).toBe(true);

    clearBrowserImageResources();
    expect(isBrowserImageReady("asset://page-0.jpg")).toBe(false);
    expect(isBrowserImageReady("asset://page-599.jpg")).toBe(false);
  });

  it("releases retained resources when the asset browser changes", () => {
    setBrowserImageResourceScope("folder-a");
    markBrowserImageReady("asset://folder-a.jpg", { width: 320, height: 240 });

    setBrowserImageResourceScope("folder-a");
    expect(isBrowserImageReady("asset://folder-a.jpg")).toBe(true);

    setBrowserImageResourceScope("folder-b");
    expect(isBrowserImageReady("asset://folder-a.jpg")).toBe(false);
  });
});
