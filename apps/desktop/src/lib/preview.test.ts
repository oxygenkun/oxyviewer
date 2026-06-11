import { describe, expect, it } from "vitest";
import { LOUPE_PREVIEW_SIZE, THUMBNAIL_PREVIEW_SIZE, previewStages } from "./preview";

describe("preview stages", () => {
  it("loads loupe previews progressively", () => {
    expect(previewStages(true)).toEqual([THUMBNAIL_PREVIEW_SIZE, LOUPE_PREVIEW_SIZE]);
  });

  it("keeps grid thumbnails on the small preview path", () => {
    expect(previewStages(false)).toEqual([THUMBNAIL_PREVIEW_SIZE]);
  });
});
