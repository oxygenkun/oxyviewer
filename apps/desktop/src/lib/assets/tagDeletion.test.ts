import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cancelPendingTagDeletions,
  enqueueTagDeletion,
  hasPendingRemoval,
  pendingDeletedTagIds,
  undoTagDeletion,
  useTagDeletions,
} from "./tagDeletion";
import { UNDO_WINDOW_MS } from "@/lib/ui/undoWindow";

const requests = () => useTagDeletions.getState().requests;

beforeEach(() => {
  vi.useFakeTimers();
  useTagDeletions.setState({ requests: [] });
});
afterEach(() => {
  cancelPendingTagDeletions();
  useTagDeletions.setState({ requests: [] });
  vi.useRealTimers();
});

it("commits a deleted tag only after the undo window", async () => {
  const commit = vi.fn(async () => {});
  enqueueTagDeletion({ kind: "tag", tagPath: "People", tagIds: new Set([1, 2]) }, commit);
  expect(pendingDeletedTagIds(requests())).toEqual(new Set([1, 2]));
  expect(requests().map(request => request.status)).toEqual(["pending"]);
  await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS - 1);
  expect(commit).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(commit).toHaveBeenCalledTimes(1);
  expect(requests()).toEqual([]);
});

it("undo cancels one photo removal without touching another photo", async () => {
  const first = vi.fn(async () => {});
  const second = vi.fn(async () => {});
  enqueueTagDeletion({ kind: "assignment", tagPath: "People", tagIds: new Set([1]), path: "/a.jpg" }, first);
  enqueueTagDeletion({ kind: "assignment", tagPath: "People", tagIds: new Set([1]), path: "/b.jpg" }, second);
  expect(hasPendingRemoval(requests(), "/a.jpg", 1)).toBe(true);
  expect(hasPendingRemoval(requests(), "/c.jpg", 1)).toBe(false);
  undoTagDeletion(requests()[0].id);
  expect(hasPendingRemoval(requests(), "/a.jpg", 1)).toBe(false);
  expect(hasPendingRemoval(requests(), "/b.jpg", 1)).toBe(true);
  await vi.advanceTimersByTimeAsync(5_000);
  expect(first).not.toHaveBeenCalled();
  expect(second).toHaveBeenCalledTimes(1);
  expect(requests()).toEqual([]);
});

it("ignores a second request that would delete the same tag again", () => {
  enqueueTagDeletion({ kind: "tag", tagPath: "People", tagIds: new Set([1]) }, async () => {});
  enqueueTagDeletion({ kind: "assignment", tagPath: "People", tagIds: new Set([1]), path: "/a.jpg" }, async () => {});
  expect(requests()).toHaveLength(1);
});

it("keeps a failed deletion visible and stops treating it as applied", async () => {
  enqueueTagDeletion({ kind: "tag", tagPath: "People", tagIds: new Set([1]) }, async () => {
    throw new Error("database is busy");
  });
  await vi.advanceTimersByTimeAsync(5_000);
  expect(requests()).toMatchObject([{ status: "failed", error: "Error: database is busy" }]);
  expect(pendingDeletedTagIds(requests()).size).toBe(0);
  undoTagDeletion(requests()[0].id);
  expect(requests()).toEqual([]);
});

it("cancels writes whose window has not expired on shutdown", async () => {
  const commit = vi.fn(async () => {});
  enqueueTagDeletion({ kind: "tag", tagPath: "People", tagIds: new Set([1]) }, commit);
  cancelPendingTagDeletions();
  await vi.advanceTimersByTimeAsync(5_000);
  expect(commit).not.toHaveBeenCalled();
  expect(requests()).toEqual([]);
});
