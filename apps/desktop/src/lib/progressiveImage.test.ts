import { describe, expect, it } from "vitest";
import { nextProgressiveStage } from "./progressiveImage";

describe("progressive image selection", () => {
  it("keeps the first available preview pending until an image has painted", () => {
    expect(nextProgressiveStage(false, ["preview", "full"])).toBe("preview");
  });

  it("allows the best available image after the preview has painted", () => {
    expect(nextProgressiveStage(true, ["thumbnail", "preview", "full"])).toBe("full");
  });

  it("falls back to the remaining preview when the full stage is unavailable", () => {
    expect(nextProgressiveStage(true, ["thumbnail", "preview", undefined])).toBe("preview");
  });
});
