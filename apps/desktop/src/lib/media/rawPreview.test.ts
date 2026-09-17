import { describe, expect, it } from "vitest";
import { rawPreviewStatus } from "./rawPreview";

describe("RAW preview status", () => {
  it("moves from preview loading through development to full resolution", () => {
    expect(rawPreviewStatus({ assetId: "raw-1", fullError: false })).toEqual({
      state: "loadingPreview",
    });
    expect(rawPreviewStatus({
      assetId: "raw-1",
      loaded: { assetId: "raw-1", mode: "preview" },
      fullError: false,
    })).toEqual({ state: "developingFull" });
    expect(rawPreviewStatus({
      assetId: "raw-1",
      loaded: { assetId: "raw-1", mode: "full" },
      fullError: false,
      fullSize: { width: 9_504, height: 6_336 },
    })).toEqual({ state: "fullReady", width: 9_504, height: 6_336 });
  });

  it("ignores a late image load from the previously selected asset", () => {
    expect(rawPreviewStatus({
      assetId: "raw-2",
      loaded: { assetId: "raw-1", mode: "full" },
      fullError: false,
      fullSize: { width: 9_504, height: 6_336 },
    })).toEqual({ state: "loadingPreview" });
  });

  it("reports full development failure while retaining the preview", () => {
    expect(rawPreviewStatus({
      assetId: "raw-1",
      loaded: { assetId: "raw-1", mode: "preview" },
      fullError: true,
    })).toEqual({ state: "fullFailed" });
  });
});
