import type { AssetTagAssignment, AssetTagAssignmentsByPath } from "@/types";
import { getAssetTagAssignmentsByPath } from "@/lib/api";

interface Waiter {
  resolve: (value: AssetTagAssignment[]) => void;
  reject: (error: unknown) => void;
  signal?: AbortSignal;
}

/** TanStack Query owns the per-path cache and mutation invalidation. This only
 * combines queries scheduled in the same turn into one asynchronous DB read. */
export function createAssetTagBatcher(
  read: (paths: string[]) => Promise<AssetTagAssignmentsByPath[]>,
) {
  let pending = new Map<string, Waiter[]>();

  async function flush() {
    const batch = pending;
    pending = new Map();
    const paths = [...batch]
      .filter(([, waiters]) => waiters.some(({ signal }) => !signal?.aborted))
      .map(([path]) => path);
    try {
      const rows = paths.length ? await read(paths) : [];
      const results = new Map(rows.map(({ path, assignments }) => [path, assignments]));
      for (const [path, waiters] of batch) {
        for (const waiter of waiters) {
          if (waiter.signal?.aborted) {
            waiter.reject(new DOMException("Cancelled", "AbortError"));
          } else {
            waiter.resolve(results.get(path) ?? []);
          }
        }
      }
    } catch (error) {
      for (const waiters of batch.values()) {
        for (const waiter of waiters) waiter.reject(error);
      }
    }
  }

  return (path: string, signal?: AbortSignal): Promise<AssetTagAssignment[]> =>
    new Promise((resolve, reject) => {
      if (signal?.aborted) {
        reject(new DOMException("Cancelled", "AbortError"));
        return;
      }
      if (!pending.size) queueMicrotask(() => { void flush(); });
      const waiters = pending.get(path) ?? [];
      waiters.push({ resolve, reject, signal });
      pending.set(path, waiters);
    });
}

export const getBatchedAssetTags = createAssetTagBatcher(getAssetTagAssignmentsByPath);
