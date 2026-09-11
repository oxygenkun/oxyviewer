// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useBackgroundAssetPagination } from "./useBackgroundAssetPagination";

type Options = Parameters<typeof useBackgroundAssetPagination>[0];
function Harness(props: Options) {
  useBackgroundAssetPagination(props);
  return null;
}

let root: ReturnType<typeof createRoot>;
let options: Options;
const render = async (changes: Partial<Options> = {}) => {
  options = { ...options, ...changes };
  await act(async () => root.render(<StrictMode><Harness {...options} /></StrictMode>));
};
const advance = async () => { await act(async () => vi.advanceTimersByTime(250)); };

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  root = createRoot(document.createElement("div"));
  options = {
    scopeKey: "folder-a", enabled: true, pageCount: 0, hasNextPage: true,
    isFetching: false, isError: false, fetchNextPage: vi.fn().mockResolvedValue(undefined),
  };
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it("waits for the first page, then keeps paging without scroll events until complete", async () => {
  await render();
  await advance();
  expect(options.fetchNextPage).not.toHaveBeenCalled();
  await render({ pageCount: 1 });
  expect(options.fetchNextPage).not.toHaveBeenCalled();
  await advance();
  expect(options.fetchNextPage).toHaveBeenCalledExactlyOnceWith({ cancelRefetch: false });
  await render({ isFetching: true });
  await advance();
  expect(options.fetchNextPage).toHaveBeenCalledTimes(1);
  await render({ pageCount: 2, isFetching: false });
  await advance();
  expect(options.fetchNextPage).toHaveBeenCalledTimes(2);
  await render({ pageCount: 3, hasNextPage: false });
  await advance();
  expect(options.fetchNextPage).toHaveBeenCalledTimes(2);
});

it("cancels the pending page when a foreground fetch starts", async () => {
  await render({ pageCount: 1 });
  await act(async () => vi.advanceTimersByTime(30));
  await render({ isFetching: true });
  await advance();
  expect(options.fetchNextPage).not.toHaveBeenCalled();
});

it("drops old directory timers and waits for the new directory's first page", async () => {
  await render({ pageCount: 1 });
  await act(async () => vi.advanceTimersByTime(30));
  const oldFetch = options.fetchNextPage;
  await render({ scopeKey: "folder-b", pageCount: 0, fetchNextPage: vi.fn().mockResolvedValue(undefined) });
  await advance();
  expect(oldFetch).not.toHaveBeenCalled();
  expect(options.fetchNextPage).not.toHaveBeenCalled();
  await render({ pageCount: 1 });
  await advance();
  expect(options.fetchNextPage).toHaveBeenCalledTimes(1);
});

it("stops after errors and leaves progressive metadata pagination to its existing owner", async () => {
  await render({ pageCount: 1, isError: true });
  await advance();
  expect(options.fetchNextPage).not.toHaveBeenCalled();
  await render({ isError: false, enabled: false });
  await advance();
  expect(options.fetchNextPage).not.toHaveBeenCalled();
});
