import { expect, it } from "vitest";
import { firstBrowseCursor, nextBrowseCursor } from "./browsePagination";

it("pins later pages to their source snapshot while refetch starts without a revision", () => {
  expect(firstBrowseCursor).toEqual({ offset: 0 });
  expect(nextBrowseCursor({ items: [], total: 500, nextCursor: 250, snapshotRevision: 0 }))
    .toEqual({ offset: 250, snapshotRevision: 0 });
  expect(nextBrowseCursor({ items: [], total: 500, nextCursor: 250, snapshotRevision: 1 }))
    .toEqual({ offset: 250, snapshotRevision: 1 });
  expect(nextBrowseCursor({ items: [], total: 0 })).toBeUndefined();
});
