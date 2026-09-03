import { describe, expect, it } from "vitest";
import { renderMethodKey, renderPlan, runtimeRenderPlatform } from "./preview";

describe("semantic render graph", () => {
  it("keeps interaction levels independent from format and pixel size", () => {
    expect(renderPlan("raw", "thumbnail", "windows").map((step) => step.level))
      .toEqual(["thumbnail"]);
    expect(renderPlan("raw", "loupe", "windows").map((step) => step.level))
      .toEqual(["preview", "full"]);
    expect(renderPlan("heif", "loupe", "windows").map((step) => step.level))
      .toEqual(["preview", "full"]);
  });

  it("maps Windows HEIF preview to the thumbnail artifact and full to tiles", () => {
    const thumbnail = renderPlan("heif", "thumbnail", "windows")[0];
    const [preview, full] = renderPlan("heif", "loupe", "windows");

    expect(preview.method).toEqual({ type: "generatedImage", requestLevel: "thumbnail" });
    expect(renderMethodKey(preview.method)).toBe(renderMethodKey(thumbnail.method));
    expect(full.method).toEqual({ type: "heifTiles" });
  });

  it("keeps the platform dimension without regressing the qualified HIF fast path", () => {
    const [preview, full] = renderPlan("heif", "loupe", "macos");
    expect(preview.method).toEqual({ type: "generatedImage", requestLevel: "thumbnail" });
    expect(full.method).toEqual({ type: "heifTiles" });
  });

  it("maps RAW and TIFF levels without exposing concrete pixel sizes", () => {
    expect(renderPlan("raw", "loupe", "windows").map((step) => step.method)).toEqual([
      { type: "generatedImage", requestLevel: "preview" },
      { type: "generatedImage", requestLevel: "full" },
    ]);
    expect(renderPlan("tiff", "loupe", "windows").map((step) => step.method)).toEqual([
      { type: "generatedImage", requestLevel: "preview" },
      { type: "generatedImage", requestLevel: "full" },
    ]);
  });

  it("uses the original image for browser-native raster formats", () => {
    expect(renderPlan("jpeg", "loupe", "windows").map((step) => step.method)).toEqual([
      { type: "originalImage" },
      { type: "originalImage" },
    ]);
  });

  it("detects the platform without making it part of interaction state", () => {
    expect(runtimeRenderPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("windows");
    expect(runtimeRenderPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)")).toBe("macos");
    expect(runtimeRenderPlatform("Mozilla/5.0 (X11; Linux x86_64)")).toBe("linux");
    expect(() => runtimeRenderPlatform("Mozilla/5.0 (FreeBSD)")).toThrow(
      "unsupported render platform",
    );
  });
});
