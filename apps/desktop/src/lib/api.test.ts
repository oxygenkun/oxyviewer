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

import { deletePaths, generatedPreview, startHeifFull } from "./api";

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

describe("HEIF full delivery", () => {
  afterEach(() => {
    mocks.invoke.mockReset();
    vi.unstubAllGlobals();
  });

  it("converts a Rust-selected artifact projection into a display URL", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    mocks.invoke.mockResolvedValue({
      delivery: "artifact",
      projection: {
        path: asset.path,
        sourceRevision: "source-1",
        projectionRevision: 1,
        validAt: 1,
        status: "ready",
        level: "full",
        result: {
          path: "/cache/full.jpg",
          width: 7008,
          height: 4672,
          kind: "decoded",
          renderLevel: "full",
        },
      },
    });

    const presentation = await startHeifFull(asset.path, 7, false);

    expect(presentation.delivery).toBe("artifact");
    if (presentation.delivery === "artifact") {
      expect(presentation.result.url).toBe("asset:///cache/full.jpg");
    }
    expect(mocks.invoke).toHaveBeenCalledWith("start_heif_full", {
      path: asset.path,
      generation: 7,
      displaySharpening: false,
    });
  });

  it("passes through a Rust-selected tile session", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    mocks.invoke.mockResolvedValue({
      delivery: "tiles",
      session: {
        id: "heif-1",
        generation: 8,
        width: 7008,
        height: 4672,
        tileSize: 512,
        expectedTiles: 140,
        backend: "appleImageIo",
        acceleration: "unknown",
        status: "decoding",
      },
    });

    await expect(startHeifFull(asset.path, 8, true)).resolves.toMatchObject({
      delivery: "tiles",
      session: { id: "heif-1" },
    });
  });
});
