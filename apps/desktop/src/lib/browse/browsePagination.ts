import type { Page } from "@/types";

export interface BrowseCursor {
  offset: number;
  snapshotRevision?: number;
}

export const firstBrowseCursor: BrowseCursor = { offset: 0 };

export function nextBrowseCursor(page: Page<unknown>): BrowseCursor | undefined {
  return page.nextCursor === undefined || page.nextCursor === null ? undefined : {
    offset: page.nextCursor,
    snapshotRevision: page.snapshotRevision,
  };
}
