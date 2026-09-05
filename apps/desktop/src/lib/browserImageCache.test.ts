import { beforeEach, describe, expect, it } from "vitest";
import {
  BROWSER_IMAGE_RESOURCE_LIMITS,
  browserImageSourceWhenEnabled,
  clearBrowserImageResources,
  isBrowserImageReady,
  firstReadyBrowserImage,
  markBrowserImageReady,
  protectBrowserImages,
  touchBrowserImage,
  setBrowserImageResourceScope,
} from "./browserImageCache";

describe("browser image resource gating", () => {
  beforeEach(() => clearBrowserImageResources());

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
    for (let index = 0; index < 600; index += 1) {
      markBrowserImageReady(`asset://page-${index}.jpg`, { width: 320, height: 240 });
    }

    expect(isBrowserImageReady("asset://page-0.jpg")).toBe(true);
    expect(isBrowserImageReady("asset://page-599.jpg")).toBe(true);

    clearBrowserImageResources();
    expect(isBrowserImageReady("asset://page-0.jpg")).toBe(false);
    expect(isBrowserImageReady("asset://page-599.jpg")).toBe(false);
  });

  it("evicts by acquisition order when the entry limit is exceeded", () => {
    for (let index = 0; index <= BROWSER_IMAGE_RESOURCE_LIMITS.maxEntries; index += 1) {
      markBrowserImageReady(`asset://entry-${index}.jpg`, { width: 1, height: 1 });
    }

    expect(isBrowserImageReady("asset://entry-0.jpg")).toBe(false);
    expect(isBrowserImageReady("asset://entry-1.jpg")).toBe(true);
    expect(isBrowserImageReady(`asset://entry-${BROWSER_IMAGE_RESOURCE_LIMITS.maxEntries}.jpg`)).toBe(true);
  });

  it("evicts by acquisition order when the decoded-size limit is exceeded", () => {
    markBrowserImageReady("asset://first-large.jpg", { width: 8_192, height: 8_192 });
    markBrowserImageReady("asset://second-large.jpg", { width: 8_192, height: 8_192 });
    markBrowserImageReady("asset://newest.jpg", { width: 1, height: 1 });

    expect(isBrowserImageReady("asset://first-large.jpg")).toBe(false);
    expect(isBrowserImageReady("asset://second-large.jpg")).toBe(true);
    expect(isBrowserImageReady("asset://newest.jpg")).toBe(true);
  });

  it("promotes a repeated resource to most recently used", () => {
    markBrowserImageReady("asset://first.jpg", { width: 1, height: 1 });
    markBrowserImageReady("asset://second.jpg", { width: 1, height: 1 });
    markBrowserImageReady("asset://first.jpg", { width: 1, height: 1 });
    for (let index = 2; index <= BROWSER_IMAGE_RESOURCE_LIMITS.maxEntries; index += 1) {
      markBrowserImageReady(`asset://entry-${index}.jpg`, { width: 1, height: 1 });
    }

    expect(isBrowserImageReady("asset://first.jpg")).toBe(true);
    expect(isBrowserImageReady("asset://second.jpg")).toBe(false);
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

describe("loupe cache retention", () => {
  beforeEach(() => clearBrowserImageResources());

  it("keeps recently displayed images ahead of untouched resources", () => {
    markBrowserImageReady("a", { width: 8192, height: 8192 });
    markBrowserImageReady("b", { width: 8192, height: 8192 });
    touchBrowserImage("a");
    markBrowserImageReady("c", { width: 1, height: 1 });
    expect(isBrowserImageReady("a")).toBe(true);
    expect(isBrowserImageReady("b")).toBe(false);
  });

  it("protects neighbors but still enforces the decoded memory budget", () => {
    protectBrowserImages(["a", "b", "c"]);
    markBrowserImageReady("a", { width: 8192, height: 8192 });
    markBrowserImageReady("background", { width: 8192, height: 8192 });
    markBrowserImageReady("b", { width: 8192, height: 8192 });
    expect(isBrowserImageReady("background")).toBe(false);
    expect(isBrowserImageReady("a")).toBe(true);
    markBrowserImageReady("c", { width: 8192, height: 8192 });
    expect(isBrowserImageReady("a")).toBe(false);
    expect(isBrowserImageReady("b")).toBe(true);
    expect(isBrowserImageReady("c")).toBe(true);
  });
});

describe("immediate progressive source", () => {
  beforeEach(() => clearBrowserImageResources());

  it("uses a decoded thumbnail until preview or full pixels are available", () => {
    const sources = ["full", "preview", "thumbnail"];
    expect(firstReadyBrowserImage(sources)).toBeUndefined();
    markBrowserImageReady("thumbnail", { width: 160, height: 120 });
    expect(firstReadyBrowserImage(sources)).toBe("thumbnail");
    markBrowserImageReady("preview", { width: 1024, height: 768 });
    expect(firstReadyBrowserImage(sources)).toBe("preview");
    markBrowserImageReady("full", { width: 2048, height: 1536 });
    expect(firstReadyBrowserImage(sources)).toBe("full");
    expect(firstReadyBrowserImage([undefined, "missing", "thumbnail"])).toBe("thumbnail");
  });
});
