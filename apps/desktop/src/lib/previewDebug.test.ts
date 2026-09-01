import { afterEach, describe, expect, it, vi } from "vitest";
import { beginPreviewDebug } from "./previewDebug";

describe("preview debug lifecycle", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("logs wait, start, completion, and the visible loading set", () => {
    const debug = vi.spyOn(console, "debug").mockImplementation(() => undefined);
    const request = beginPreviewDebug({
      assetName: "screen-photo.CR3",
      stage: "thumbnail@512",
      priority: "visible",
    });

    request?.start();
    request?.mark("first-pixel", { pixels: 1 });

    expect(debug).toHaveBeenCalledWith(expect.stringContaining("[WAIT] screen-photo.CR3"));
    expect(debug).toHaveBeenCalledWith(
      "[OxyPreview][STATE]",
      expect.objectContaining({ visibleLoading: ["screen-photo.CR3"] }),
    );
    expect(debug).toHaveBeenCalledWith(
      expect.stringContaining("[MARK] screen-photo.CR3 | thumbnail@512 | visible | first-pixel"),
      { pixels: 1 },
    );

    request?.complete({ backend: "fixture" });
    expect(debug).toHaveBeenCalledWith(
      expect.stringMatching(/\[DONE\] screen-photo\.CR3.*wait=.*load=.*total=/),
      { backend: "fixture" },
    );
  });
});
