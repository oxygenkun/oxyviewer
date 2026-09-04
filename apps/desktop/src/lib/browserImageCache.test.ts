import { describe, expect, it } from "vitest";
import {
  browserImageSourceWhenEnabled,
  markBrowserImageReady,
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
});
