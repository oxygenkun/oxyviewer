import { describe, expect, it } from "vitest";
import {
  loupeThumbnailFallback,
  renderMethodKey,
  renderPlan,
  runtimeRenderPlatform,
} from "./preview";

describe("semantic render graph", () => {
  it("keeps interaction levels independent from format and pixel size", () => {
    expect(renderPlan("raw", "thumbnail", "windows").map((step) => step.level))
      .toEqual(["thumbnail"]);
    expect(renderPlan("raw", "loupe", "windows").map((step) => step.level))
      .toEqual(["preview", "full"]);
    expect(renderPlan("heif", "loupe", "windows").map((step) => step.level))
      .toEqual(["preview", "full"]);
  });

  it("keeps HEIF thumbnail and preview requests distinct while full uses tiles", () => {
    const thumbnail = renderPlan("heif", "thumbnail", "windows")[0];
    const [preview, full] = renderPlan("heif", "loupe", "windows");

    expect(preview.method).toEqual({ type: "generatedImage", requestLevel: "preview" });
    expect(renderMethodKey(preview.method)).not.toBe(renderMethodKey(thumbnail.method));
    expect(full.method).toEqual({ type: "heifFull" });
  });

  it("delegates HEIF full delivery to Rust on every platform", () => {
    for (const platform of ["windows", "macos", "linux"] as const) {
      const [preview, full] = renderPlan("heif", "loupe", platform);
      expect(preview.method).toEqual({ type: "generatedImage", requestLevel: "preview" });
      expect(full.method).toEqual({ type: "heifFull" });
    }
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

  it("reuses filmstrip artifacts when loupe previews are distinct", () => {
    expect(loupeThumbnailFallback("raw", "windows")).toEqual({
      level: "thumbnail",
      method: { type: "generatedImage", requestLevel: "thumbnail" },
    });
    expect(loupeThumbnailFallback("tiff", "windows")?.level).toBe("thumbnail");
    expect(loupeThumbnailFallback("heif", "windows")?.level).toBe("thumbnail");
    expect(loupeThumbnailFallback("jpeg", "windows")).toBeUndefined();
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
