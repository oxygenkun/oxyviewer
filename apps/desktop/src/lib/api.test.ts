import { afterEach, describe, expect, it, vi } from "vitest";
import type { AssetSummary } from "../types";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
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
  beginPreviewDebug: vi.fn(() => undefined),
}));

import { generatedPreview } from "./api";

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
});
