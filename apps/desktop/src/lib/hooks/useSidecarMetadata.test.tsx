// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AssetSummary } from "@/types";
import { requestMetadata } from "@/lib/api";
import { useSidecarMetadata } from "./useSidecarMetadata";

vi.mock("@/lib/api", () => ({ requestMetadata: vi.fn().mockResolvedValue([]) }));
const asset = (id: number, hasSidecar = true): AssetSummary => ({
  id: String(id), path: `/photos/${id}.HIF`, name: `${id}.HIF`, extension: "HIF",
  kind: "heif", sizeBytes: 100, modifiedAtMs: 1, hasSidecar,
});
function Harness({ scope, assets }: { scope: string; assets: AssetSummary[] }) {
  useSidecarMetadata(scope, assets);
  return null;
}
let root: ReturnType<typeof createRoot>;
const render = async (assets: AssetSummary[], scope = "folder") => {
  await act(async () => root.render(<StrictMode><Harness assets={assets} scope={scope} /></StrictMode>));
};
const advance = async () => { await act(async () => vi.advanceTimersByTime(200)); };
beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  vi.mocked(requestMetadata).mockResolvedValue([]);
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  root = createRoot(document.createElement("div"));
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it("reads sidecars after painting and admits bounded background batches", async () => {
  const assets = Array.from({ length: 34 }, (_, id) => asset(id));
  await render([...assets, asset(99, false)]);
  expect(requestMetadata).not.toHaveBeenCalled();
  await advance();
  expect(requestMetadata).toHaveBeenCalledExactlyOnceWith(assets.slice(0, 32).map(({ path }) => path), "background");
  await advance();
  expect(requestMetadata).toHaveBeenLastCalledWith(assets.slice(32).map(({ path }) => path), "background");
});

it("requests only new paged sidecars and cancels timers on directory changes", async () => {
  await render([asset(1)]);
  await advance();
  await render([asset(1), asset(2)]);
  await advance();
  expect(requestMetadata).toHaveBeenLastCalledWith([asset(2).path], "background");
  await render([asset(1), asset(2), asset(3)]);
  await render([asset(4)], "new folder");
  await advance();
  expect(requestMetadata).toHaveBeenCalledTimes(3);
  expect(requestMetadata).toHaveBeenLastCalledWith([asset(4).path], "background");
});

it("stops after a failed batch and allows a later listing to retry", async () => {
  vi.mocked(requestMetadata).mockRejectedValueOnce(new Error("unavailable"));
  await render([asset(1)]);
  await advance();
  await advance();
  expect(requestMetadata).toHaveBeenCalledTimes(1);
  await render([asset(1)]);
  await advance();
  expect(requestMetadata).toHaveBeenCalledTimes(2);
});

it("rereads the same paths after the directory snapshot is refreshed", async () => {
  await render([asset(1)], "folder:revision:1");
  await advance();
  await render([asset(1)], "folder:revision:2");
  await advance();
  expect(requestMetadata).toHaveBeenCalledTimes(2);
  expect(requestMetadata).toHaveBeenLastCalledWith([asset(1).path], "background");
});
