// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  aggregatePreviewDebugSnapshot,
  beginPreviewDebug,
  getPreviewDebugSnapshot,
  setPreviewDebugLoggingEnabled,
  type TrackedPreview,
} from "./previewDebug";

describe("preview debug lifecycle", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    setPreviewDebugLoggingEnabled(false);
  });
  afterEach(() => {
    vi.runOnlyPendingTimers();
    vi.useRealTimers();
    setPreviewDebugLoggingEnabled(false);
    vi.restoreAllMocks();
  });

  it("logs wait, start, completion, and the visible loading set", () => {
    const debug = vi.spyOn(console, "debug").mockImplementation(() => undefined);
    setPreviewDebugLoggingEnabled(true);
    const request = beginPreviewDebug({
      assetName: "screen-photo.CR3",
      stage: "thumbnail@512",
      priority: "visible",
    });

    request?.start();
    request?.mark("first-pixel", { pixels: 1 });
    vi.advanceTimersByTime(250);

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

  it("coalesces a grid burst while keeping the local snapshot current", () => {
    const debug = vi.spyOn(console, "debug").mockImplementation(() => undefined);
    const writes = vi.spyOn(Storage.prototype, "setItem");
    const requests = Array.from({ length: 80 }, (_, index) => beginPreviewDebug({
      assetName: `grid-${index}.HIF`, stage: "thumbnail", priority: "visible",
    }));
    requests.forEach((request) => request?.start());
    expect(getPreviewDebugSnapshot().loading).toHaveLength(80);
    expect(writes).not.toHaveBeenCalled();
    expect(debug).not.toHaveBeenCalled();
    vi.advanceTimersByTime(250);
    expect(writes).toHaveBeenCalledTimes(1);
    requests.forEach((request) => request?.complete());
    expect(getPreviewDebugSnapshot()).toEqual({ waiting: [], loading: [] });
    vi.advanceTimersByTime(250);
    expect(writes).toHaveBeenCalledTimes(2);
    expect(JSON.parse(writes.mock.calls[1][1])).toEqual({ waiting: [], loading: [] });
  });

  it("keeps requests working when the cross-window storage is unavailable", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
    const request = beginPreviewDebug({ assetName: "one.HIF", stage: "thumbnail", priority: "visible" });
    request?.start();
    expect(() => vi.advanceTimersByTime(250)).not.toThrow();
    request?.complete();
    expect(getPreviewDebugSnapshot()).toEqual({ waiting: [], loading: [] });
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
