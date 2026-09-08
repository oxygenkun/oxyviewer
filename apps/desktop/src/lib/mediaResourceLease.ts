import { releaseMediaResource } from "./api";

// Projection/query results can be shared by a thumbnail and the loupe. The
// backend lease is per resource, not per component: only the last local owner
// may release it. Renewal and expired-resource recovery stay with each owner.
const owners = new Map<string, number>();
const pendingReleases = new Set<string>();

export function retainMediaResource(resourceId: string): () => void {
  owners.set(resourceId, (owners.get(resourceId) ?? 0) + 1);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    const remaining = (owners.get(resourceId) ?? 1) - 1;
    if (remaining > 0) {
      owners.set(resourceId, remaining);
    } else {
      owners.delete(resourceId);
      releaseUnretainedMediaResource(resourceId);
    }
  };
}

/** Also handles a result returned after its requesting component was disposed. */
export function releaseUnretainedMediaResource(resourceId: string): void {
  if (owners.has(resourceId) || pendingReleases.has(resourceId)) return;
  pendingReleases.add(resourceId);
  // StrictMode and effect replacement can relinquish and reclaim a resource
  // within one turn. Do not send a release between those two ownership changes.
  queueMicrotask(() => {
    pendingReleases.delete(resourceId);
    if (!owners.has(resourceId)) {
      // Teardown must not produce an unhandled rejection during app shutdown.
      // If IPC fails, the backend's expiring lease remains the fallback.
      void releaseMediaResource(resourceId).catch(() => {});
    }
  });
}
