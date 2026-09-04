import { afterEach, describe, expect, it, vi } from "vitest";
import {
  aggregatePreviewDebugSnapshot,
  beginPreviewDebug,
  type TrackedPreview,
} from "./previewDebug";

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

  it("shows one row with a consumer count for duplicate resource requests", () => {
    const request = (overrides: Partial<TrackedPreview>): TrackedPreview => ({
      id: 1,
      assetName: "DSC04726.HIF",
      stage: "image-decode",
      priority: "visible",
      resourceKey: "image:asset://localhost/preview.jpg",
      resourceLabel: "thumbnail",
      queuedAt: 10,
      ...overrides,
    });

    const snapshot = aggregatePreviewDebugSnapshot({
      waiting: [request({ id: 1 })],
      loading: [request({ id: 2, priority: "loupe", queuedAt: 20, startedAt: 30 })],
    });

    expect(snapshot.waiting).toEqual([]);
    expect(snapshot.loading).toHaveLength(1);
    expect(snapshot.loading[0]).toEqual(expect.objectContaining({
      assetName: "DSC04726.HIF",
      priority: "loupe",
      consumers: 2,
      resourceLabel: "thumbnail",
      queuedAt: 10,
      startedAt: 30,
    }));
  });
});
