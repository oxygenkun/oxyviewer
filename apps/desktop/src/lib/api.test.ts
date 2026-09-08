import { afterEach, describe, expect, it, vi } from "vitest";
import type { AssetSummary } from "../types";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  beginPreviewDebug: vi.fn(() => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset://${path}`,
  invoke: mocks.invoke,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("./previewDebug", () => ({
  beginPreviewDebug: mocks.beginPreviewDebug,
}));

import { deletePaths, generatedPreview, heifTileUrl, startHeifFull } from "./api";
import {
  acceptImageProjection,
  clearImageProjections,
  imageProjectionKey,
  useImageProjectionStore,
} from "./imageProjection";
import { mediaProtocolUrl } from "./mediaProtocolUrl";

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
    clearImageProjections();
    mocks.invoke.mockReset();
    mocks.listen.mockReset();
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

  it("releases a resource whose IPC result arrives after cancellation", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    let resolve!: (value: unknown) => void;
    mocks.invoke.mockImplementation((command: string) => command === "get_preview"
      ? new Promise((done) => { resolve = done; }) : Promise.resolve(true));
    const controller = new AbortController();
    const request = generatedPreview(asset, "thumbnail", controller.signal);
    controller.abort();
    await expect(request).rejects.toMatchObject({ name: "AbortError" });
    resolve({ result: { resource: { resourceId: "late" } } });
    await new Promise((done) => setTimeout(done, 0));
    expect(mocks.invoke).toHaveBeenCalledWith("release_media_resource", { resourceId: "late" });
  });

  it("cancels a pending HEIF artifact request and releases its late result", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    let resolve!: (value: unknown) => void;
    mocks.invoke.mockImplementation((command: string) => command === "start_heif_full"
      ? new Promise((done) => { resolve = done; }) : Promise.resolve(true));
    const controller = new AbortController();
    const request = startHeifFull(asset.path, 5, false, controller.signal);
    const requestId = mocks.invoke.mock.calls.find(([command]) => command === "start_heif_full")![1].requestId;
    controller.abort();
    expect(mocks.invoke).toHaveBeenCalledWith("cancel_preview_request", { path: asset.path, level: "full", requestId });
    resolve({ delivery: "artifact", projection: { result: { resource: { resourceId: "late-full" } } } });
    await expect(request).rejects.toMatchObject({ name: "AbortError" });
    await new Promise((done) => setTimeout(done, 0));
    expect(mocks.invoke).toHaveBeenCalledWith("release_media_resource", { resourceId: "late-full" });
  });

  it("uses a registered media resource before its managed path exists", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    mocks.invoke.mockResolvedValue({
      path: asset.path,
      sourceRevision: "revision",
      projectionRevision: 1,
      validAt: 1,
      status: "ready",
      level: "thumbnail",
      result: {
        path: "C:\\cache\\pending.jpg",
        width: 160,
        height: 120,
        kind: "embedded",
        renderLevel: "thumbnail",
        resource: {
          resourceId: "resource-1",
          url: "oxy-media://localhost/resource/resource-1",
          mediaType: "image/jpeg",
        },
        satisfaction: "satisfied",
        persistence: "pending",
      },
    });

    await expect(generatedPreview(asset, "thumbnail")).resolves.toMatchObject({
      url: "oxy-media://localhost/resource/resource-1",
      persistence: "pending",
    });
  });

  it("settles a displayable Interim preview without hiding it behind its upgrade", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    mocks.invoke.mockResolvedValue({
      path: asset.path,
      sourceRevision: "interim-revision",
      projectionRevision: 1,
      validAt: 1,
      status: "ready",
      level: "preview",
      result: {
        path: "C:\\cache\\interim.jpg",
        width: 160,
        height: 120,
        kind: "embedded",
        renderLevel: "preview",
        resource: {
          resourceId: "resource-interim",
          url: "oxy-media://localhost/resource/resource-interim",
          mediaType: "image/jpeg",
        },
        satisfaction: "interim",
        persistence: "pending",
      },
    });
    const controller = new AbortController();

    await expect(generatedPreview(asset, "preview", controller.signal)).resolves.toMatchObject({
      satisfaction: "interim",
      url: "oxy-media://localhost/resource/resource-interim",
    });
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "cancel_preview_request",
      expect.anything(),
    );
  });

  it("retains Interim pixels when native publishes a terminal upgrade failure", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "cancel_preview_request") return Promise.resolve(true);
      if (command === "get_preview") {
        return Promise.resolve({
          path: asset.path,
          sourceRevision: "upgrade-failure-revision",
          projectionRevision: 1,
          validAt: 1,
          status: "ready",
          level: "preview",
          result: {
            path: "C:\\cache\\interim.jpg",
            width: 160,
            height: 120,
            kind: "embedded",
            renderLevel: "preview",
            resource: {
              resourceId: "resource-interim-failure",
              url: "oxy-media://localhost/resource/resource-interim-failure",
              mediaType: "image/jpeg",
            },
            satisfaction: "interim",
            persistence: "pending",
          },
        });
      }
      return Promise.reject(new Error(`unexpected command ${command}`));
    });
    const controller = new AbortController();
    await expect(generatedPreview(asset, "preview", controller.signal)).resolves.toMatchObject({
      satisfaction: "interim",
    });

    acceptImageProjection({
      path: asset.path,
      sourceRevision: "upgrade-failure-revision",
      projectionRevision: 2,
      validAt: 1,
      status: "error",
      level: "preview",
      error: "decoder failed",
    });

    expect(useImageProjectionStore.getState().records[
      imageProjectionKey(asset.path, "preview")
    ]).toMatchObject({
      status: "error",
      error: "decoder failed",
      result: {
        satisfaction: "interim",
        resource: { resourceId: "resource-interim-failure" },
      },
    });
  });

  it("settles an undersized Interim thumbnail because it has no upgrade phase", async () => {
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    mocks.invoke.mockResolvedValue({
      path: asset.path,
      sourceRevision: "thumbnail-interim-revision",
      projectionRevision: 1,
      validAt: 1,
      status: "ready",
      level: "thumbnail",
      result: {
        path: "C:\\cache\\thumbnail-interim.jpg",
        width: 160,
        height: 120,
        kind: "embedded",
        renderLevel: "thumbnail",
        resource: {
          resourceId: "resource-thumbnail-interim",
          url: "oxy-media://localhost/resource/resource-thumbnail-interim",
          mediaType: "image/jpeg",
        },
        satisfaction: "interim",
        persistence: "pending",
      },
    });
    const controller = new AbortController();

    await expect(generatedPreview(asset, "thumbnail", controller.signal)).resolves.toMatchObject({
      satisfaction: "interim",
      url: "oxy-media://localhost/resource/resource-thumbnail-interim",
    });
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "cancel_preview_request",
      expect.anything(),
    );
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

describe("media protocol URL normalization", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("uses the WebView2 HTTP form for resources and tiles on Windows", () => {
    vi.stubGlobal("navigator", { userAgent: "Windows" });
    const controlled = "oxy-media://localhost/resource/resource-1";

    expect(mediaProtocolUrl(controlled)).toBe(
      "http://oxy-media.localhost/resource/resource-1",
    );
    expect(heifTileUrl("oxy-media://localhost/tile/session/1/0/0")).toBe(
      "http://oxy-media.localhost/tile/session/1/0/0",
    );
  });

  it("leaves the custom scheme unchanged outside Windows", () => {
    vi.stubGlobal("navigator", { userAgent: "Macintosh" });
    expect(mediaProtocolUrl("oxy-media://localhost/resource/resource-1")).toBe(
      "oxy-media://localhost/resource/resource-1",
    );
  });
});

describe("HEIF full delivery", () => {
  afterEach(() => {
    mocks.invoke.mockReset();
    vi.unstubAllGlobals();
  });

  it("uses the controlled resource URL selected by Rust for a full artifact", async () => {
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
          resource: {
            resourceId: "full-resource",
            url: "oxy-media://localhost/resource/full-resource",
            mediaType: "image/jpeg",
          },
        },
      },
    });

    const presentation = await startHeifFull(asset.path, 7, false);

    expect(presentation.delivery).toBe("artifact");
    if (presentation.delivery === "artifact") {
      expect(presentation.result.url).toBe(
        "oxy-media://localhost/resource/full-resource",
      );
    }
    expect(mocks.invoke).toHaveBeenCalledWith("start_heif_full", {
      requestId: expect.any(String),
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
