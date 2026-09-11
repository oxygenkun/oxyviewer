import { describe, expect, it } from "vitest";
import {
  renderMethodKey,
  renderPlan,
  runtimeRenderPlatform,
} from "./preview";

describe("semantic render graph", () => {
  it("keeps interaction levels independent from format and pixel size", () => {
    expect(renderPlan("raw", "thumbnail", "windows").map((step) => step.level))
      .toEqual(["thumbnail"]);
    expect(renderPlan("raw", "loupe", "windows").map((step) => step.level))
      .toEqual(["thumbnail", "full"]);
    expect(renderPlan("heif", "loupe", "windows").map((step) => step.level))
      .toEqual(["thumbnail", "full"]);
  });

  it("reuses the HEIF thumbnail in loupe while full uses tiles", () => {
    const thumbnail = renderPlan("heif", "thumbnail", "windows")[0];
    const [base, full] = renderPlan("heif", "loupe", "windows");

    expect(base.method).toEqual({ type: "generatedImage", requestLevel: "thumbnail" });
    expect(renderMethodKey(base.method)).toBe(renderMethodKey(thumbnail.method));
    expect(full.method).toEqual({ type: "heifFull" });
  });

  it("delegates HEIF full delivery to Rust on every platform", () => {
    for (const platform of ["windows", "macos", "linux"] as const) {
      const [base, full] = renderPlan("heif", "loupe", platform);
      expect(base.method).toEqual({ type: "generatedImage", requestLevel: "thumbnail" });
      expect(full.method).toEqual({ type: "heifFull" });
    }
  });

  it("maps RAW and TIFF levels without exposing concrete pixel sizes", () => {
    expect(renderPlan("raw", "loupe", "windows").map((step) => step.method)).toEqual([
      { type: "generatedImage", requestLevel: "thumbnail" },
      { type: "generatedImage", requestLevel: "full" },
    ]);
    expect(renderPlan("tiff", "loupe", "windows").map((step) => step.method)).toEqual([
      { type: "generatedImage", requestLevel: "thumbnail" },
      { type: "generatedImage", requestLevel: "full" },
    ]);
  });

  it("requests full independently for browser-native raster formats", () => {
    expect(renderPlan("jpeg", "loupe", "windows").map((step) => step.method)).toEqual([
      { type: "generatedImage", requestLevel: "thumbnail" },
      { type: "generatedImage", requestLevel: "full" },
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
