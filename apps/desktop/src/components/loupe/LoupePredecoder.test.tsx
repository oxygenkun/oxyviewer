// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AssetSummary } from "@/types";
import { LoupePredecoder } from "./LoupePredecoder";

const mock = vi.hoisted(() => ({ decode: vi.fn(), heif: vi.fn() }));
vi.mock("@/lib/preview/fullPredecode", () => ({ predecodeFullImage: mock.decode }));
vi.mock("@/lib/api", () => ({ isTauri: () => true }));
vi.mock("./HeifTileCanvas", () => ({ HeifTileCanvas: (props: unknown) => { mock.heif(props); return null; } }));
const asset = (id: string, kind = "jpeg") => ({ id, path: `/photos/${id}`, modifiedAtMs: 1, kind } as AssetSummary);
let root: Root;
let host: HTMLDivElement;
let pending: Array<{ signal: AbortSignal; resolve: () => void }>;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  pending = [];
  mock.decode.mockReset().mockImplementation((_asset: AssetSummary, signal: AbortSignal) => new Promise<void>((resolve) => pending.push({ signal, resolve })));
  mock.heif.mockReset();
  host = document.createElement("div");
  root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); vi.unstubAllGlobals(); });
const render = async (assets: AssetSummary[], ready: boolean) => {
  await act(async () => root.render(<LoupePredecoder assets={assets} ready={ready} displaySharpening />));
};
it("waits for the current presentation and advances one full image at a time", async () => {
  const assets = [asset("a"), asset("b"), asset("c")];
  await render(assets, false);
  expect(mock.decode).not.toHaveBeenCalled();
  await render(assets, true);
  expect(mock.decode).toHaveBeenCalledTimes(1);
  await act(async () => pending[0].resolve());
  expect(mock.decode).toHaveBeenCalledTimes(2);
  expect(mock.decode.mock.calls[1][0].id).toBe("b");
  await act(async () => pending[1].resolve());
  await act(async () => pending[2].resolve());
  expect(mock.decode).toHaveBeenCalledTimes(3);
});
it("cancels a removed candidate and prevents its late completion from advancing work", async () => {
  await render([asset("a"), asset("b")], true);
  await render([], true);
  expect(pending[0].signal.aborted).toBe(true);
  await act(async () => pending[0].resolve());
  expect(mock.decode).toHaveBeenCalledTimes(1);
});
it("waits for HEIF presentation rather than native completion", async () => {
  await render([asset("a", "heif"), asset("b")], true);
  const props = mock.heif.mock.calls.at(-1)![0];
  expect(props.priority).toBe("preload");
  await act(async () => props.onStatus("complete"));
  expect(mock.decode).not.toHaveBeenCalled();
  await act(async () => props.onPresentationReady());
  expect(mock.decode).toHaveBeenCalledTimes(1);
});
