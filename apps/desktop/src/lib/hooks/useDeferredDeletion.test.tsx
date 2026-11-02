// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AssetSummary } from "@/types";
import { useDeferredDeletion } from "./useDeferredDeletion";

let root: ReturnType<typeof createRoot>;
let deletion: ReturnType<typeof useDeferredDeletion>;
function Harness() { deletion = useDeferredDeletion(); return null; }
const asset = (path: string) => ({ path } as AssetSummary);
beforeEach(async () => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  root = createRoot(document.createElement("div"));
  await act(async () => root.render(<Harness />));
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
it("hides immediately and commits only after five seconds", async () => {
  const commit = vi.fn().mockResolvedValue(undefined);
  await act(async () => deletion.enqueue([asset("a")], commit));
  expect(deletion.hiddenPaths.has("a")).toBe(true);
  await act(async () => vi.advanceTimersByTime(4999));
  expect(commit).not.toHaveBeenCalled();
  await act(async () => vi.advanceTimersByTime(1));
  expect(commit).toHaveBeenCalledTimes(1);
  expect(deletion.hiddenPaths.size).toBe(0);
});
it("undo restores one batch without cancelling another", async () => {
  const first = vi.fn();
  const second = vi.fn().mockResolvedValue(undefined);
  await act(async () => deletion.enqueue([asset("a")], first));
  const id = deletion.pending[0].id;
  await act(async () => deletion.enqueue([asset("b")], second));
  await act(async () => deletion.undo(id));
  expect(deletion.hiddenPaths.has("a")).toBe(false);
  expect(deletion.hiddenPaths.has("b")).toBe(true);
  await act(async () => vi.advanceTimersByTime(5000));
  expect(first).not.toHaveBeenCalled();
  expect(second).toHaveBeenCalledTimes(1);
});
it("cancels pending disk operations when unmounted", async () => {
  const commit = vi.fn();
  await act(async () => deletion.enqueue([asset("a")], commit));
  await act(async () => root.unmount());
  vi.advanceTimersByTime(5000);
  expect(commit).not.toHaveBeenCalled();
});
