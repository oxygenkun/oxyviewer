import { afterEach, describe, expect, it, vi } from "vitest";
import type { AssetSummary } from "../types";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  beginPreviewDebug: vi.fn(() => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset://${path}`,
  invoke: mocks.invoke,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("./previewDebug", () => ({
  beginPreviewDebug: mocks.beginPreviewDebug,
}));

import { deletePaths, generatedPreview } from "./api";

const asset: AssetSummary = {
  id: "raw-1",
  path: "C:\\photos\\raw-1.ARW",
  name: "raw-1.ARW",
  extension: "arw",
  kind: "raw",
  sizeBytes: 1,
  modifiedAtMs: 1,
  hasSidecar: false,
};

describe("generated preview cancellation", () => {
  afterEach(() => {
    mocks.invoke.mockReset();
    mocks.beginPreviewDebug.mockClear();
    vi.unstubAllGlobals();
  });

  it("releases a backend request that scrolls out before it starts", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_preview") return new Promise(() => undefined);
      if (command === "cancel_preview_request") return Promise.resolve(true);
      return Promise.reject(new Error(`unexpected command ${command}`));
    });
    const controller = new AbortController();
    const request = generatedPreview(asset, "thumbnail", controller.signal, "visible", 0);

    controller.abort();

    await expect(request).rejects.toMatchObject({ name: "AbortError" });
    expect(mocks.invoke).toHaveBeenCalledWith("cancel_preview_request", {
      path: asset.path,
      level: "thumbnail",
      requestId: expect.any(String),
    });
  });

  it("does not leave a debug WAIT entry for an already-cancelled request", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    const controller = new AbortController();
    controller.abort();

    await expect(generatedPreview(asset, "thumbnail", controller.signal))
      .rejects.toMatchObject({ name: "AbortError" });

    expect(mocks.beginPreviewDebug).not.toHaveBeenCalled();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});

describe("file deletion", () => {
  afterEach(() => {
    mocks.invoke.mockReset();
    vi.unstubAllGlobals();
  });

  it.each([
    ["trash", "trash"],
    ["permanent", "deletePermanently"],
  ] as const)("maps %s mode to the native %s operation", async (mode, type) => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    mocks.invoke.mockResolvedValue(undefined);

    await deletePaths([asset.path], mode);

    expect(mocks.invoke).toHaveBeenCalledWith("execute_file_operation", {
      operation: { type, paths: [asset.path] },
    });
  });
});
