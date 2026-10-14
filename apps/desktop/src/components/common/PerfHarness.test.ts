import { beforeEach, describe, expect, it, vi } from "vitest";

describe("full HEIF performance completion", () => {
  beforeEach(() => vi.resetModules());

  it("accepts a displayed artifact or a fully painted native frame for the selected photo", async () => {
    const { markSatisfied } = await import("./PerfHarness");
    const { activatePerfProbe, perfMark } = await import("@/lib/diagnostics/perfProbe");
    activatePerfProbe();
    const token = "image:loaded@full|heif:all-tiles-painted";
    const scenario = { selectName: "photo.HIF", scrollToEnd: false };
    perfMark("image:loaded", { assetName: "other.HIF", large: true, stage: "full" });
    perfMark("image:loaded", { assetName: "photo.HIF", large: true, stage: "thumbnail" });
    expect(markSatisfied(token, scenario)).toBe(false);
    perfMark("heif:all-tiles-painted", { assetName: "photo.HIF", allSettled: true, failed: 1, canvasVisibility: "visible" });
    expect(markSatisfied(token, scenario)).toBe(false);
    perfMark("heif:all-tiles-painted", { assetName: "photo.HIF", allSettled: true, failed: 0, canvasVisibility: "hidden" });
    expect(markSatisfied(token, scenario)).toBe(false);
    perfMark("heif:all-tiles-painted", { assetName: "photo.HIF", allSettled: true, failed: 0, canvasVisibility: "visible" });
    expect(markSatisfied(token, scenario)).toBe(true);
  });

  it("accepts a full cached image independently of native tile marks", async () => {
    const { markSatisfied } = await import("./PerfHarness");
    const { activatePerfProbe, perfMark } = await import("@/lib/diagnostics/perfProbe");
    activatePerfProbe();
    perfMark("image:loaded", { assetName: "photo.HIF", large: true, stage: "full" });
    expect(markSatisfied("image:loaded@full|heif:all-tiles-painted", { selectName: "photo.HIF", scrollToEnd: false })).toBe(true);
  });
});
