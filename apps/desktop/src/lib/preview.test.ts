import { describe, expect, it } from "vitest";
import { LOUPE_PREVIEW_SIZE, THUMBNAIL_PREVIEW_SIZE, previewStages } from "./preview";

describe("preview stages", () => {
  it("keeps grid thumbnails on the small preview path for every format", () => {
    expect(previewStages("jpeg", false)).toEqual([THUMBNAIL_PREVIEW_SIZE]);
    expect(previewStages("raw", false)).toEqual([THUMBNAIL_PREVIEW_SIZE]);
    expect(previewStages("heif", false)).toEqual([THUMBNAIL_PREVIEW_SIZE]);
  });

  it("keeps only a 512 placeholder for HEIF loupe while tiles provide full detail", () => {
    expect(previewStages("heif", true)).toEqual([THUMBNAIL_PREVIEW_SIZE]);
  });

  it("upgrades raw loupe through 512, 4096, then full detail", () => {
    expect(previewStages("raw", true)).toEqual([
      THUMBNAIL_PREVIEW_SIZE,
      LOUPE_PREVIEW_SIZE,
      "full",
    ]);
  });

  it("defaults future formats to the two-tier preview path", () => {
    expect(previewStages("tiff", true)).toEqual([
      THUMBNAIL_PREVIEW_SIZE,
      LOUPE_PREVIEW_SIZE,
    ]);
  });
});
