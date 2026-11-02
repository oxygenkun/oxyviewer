// @vitest-environment jsdom
import { notifyManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { getTagSyncStatus, onTagSyncStatusChanged, retryTagXmpSync } from "@/lib/api";
import { TagSyncIndicator } from "./TagSyncIndicator";

vi.mock("@/lib/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...original,
    getTagSyncStatus: vi.fn(async () => ({ pendingCount: 0, failedCount: 0, lastError: null })),
    onTagSyncStatusChanged: vi.fn(async () => () => {}),
    retryTagXmpSync: vi.fn(async () => {}),
  };
});

let host: HTMLDivElement;
let root: Root;
let client: QueryClient;

const indicator = () => host.querySelector<HTMLButtonElement>(".tag-sync-indicator")!;
const render = async () => {
  await act(async () => root.render(
    <QueryClientProvider client={client}>
      <TagSyncIndicator t={(key) => key} />
    </QueryClientProvider>,
  ));
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
};
const settle = async () => {
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
};

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  notifyManager.setNotifyFunction((callback) => { act(callback); });
  vi.mocked(getTagSyncStatus).mockClear().mockResolvedValue({ pendingCount: 0, failedCount: 0, lastError: null });
  vi.mocked(onTagSyncStatusChanged).mockClear().mockResolvedValue(() => {});
  vi.mocked(retryTagXmpSync).mockClear().mockResolvedValue(undefined);
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  client.clear();
  notifyManager.setNotifyFunction((callback) => { callback(); });
  vi.unstubAllGlobals();
});

it("keeps an empty mirror queue hidden and unfocusable", async () => {
  await render();
  expect(indicator().dataset.state).toBe("idle");
  expect(indicator().getAttribute("aria-hidden")).toBe("true");
  expect(indicator().tabIndex).toBe(-1);
});

it("follows queued and drained snapshots pushed by the queue", async () => {
  await render();
  const push = vi.mocked(onTagSyncStatusChanged).mock.calls[0]![0];
  expect(indicator().dataset.state).toBe("idle");

  await act(async () => push({ pendingCount: 2, failedCount: 0, lastError: null }));
  await settle();
  expect(indicator().dataset.state).toBe("pending");
  expect(indicator().getAttribute("aria-hidden")).toBe("false");
  expect(indicator().title).toBe("tagSyncPending");

  await act(async () => push({ pendingCount: 0, failedCount: 0, lastError: null }));
  await settle();
  expect(indicator().dataset.state).toBe("idle");
  expect(indicator().getAttribute("aria-hidden")).toBe("true");
});

it("retries a failed mirror write from the same slot", async () => {
  vi.mocked(getTagSyncStatus).mockResolvedValue({ pendingCount: 1, failedCount: 1, lastError: "disk full" });
  await render();
  expect(indicator().dataset.state).toBe("error");
  expect(indicator().tabIndex).toBe(0);

  await act(async () => indicator().click());
  await settle();
  expect(retryTagXmpSync).toHaveBeenCalledTimes(1);
});
