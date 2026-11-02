import { useEffect, useMemo, useRef, useState } from "react";
import type { AssetSummary } from "@/types";

type PendingDeletion = {
  id: number;
  expired?: boolean;
  assets: AssetSummary[];
  timer: ReturnType<typeof setTimeout>;
};

/** Pending files remain on disk; unmount cancels requests that have not started. */
export function useDeferredDeletion() {
  const [pending, setPending] = useState<PendingDeletion[]>([]);
  const requests = useRef(new Map<number, PendingDeletion>());
  const nextId = useRef(0);
  useEffect(() => () => {
    for (const request of requests.current.values()) clearTimeout(request.timer);
    requests.current.clear();
  }, []);
  const hiddenPaths = useMemo(() => new Set(pending.flatMap(request => request.assets.map(asset => asset.path))), [pending]);
  const remove = (id: number) => {
    requests.current.delete(id);
    setPending(current => current.filter(request => request.id !== id));
  };
  const enqueue = (assets: AssetSummary[], commit: () => Promise<void>) => {
    const reserved = new Set([...requests.current.values()].flatMap(request => request.assets.map(asset => asset.path)));
    if (!assets.length || assets.some(asset => reserved.has(asset.path))) return;
    const id = nextId.current++;
    const timer = setTimeout(() => {
      // Keep the optimistic projection until cache reconciliation finishes.
      const request = requests.current.get(id);
      if (request) request.expired = true;
      setPending(current => current.map(request => request.id === id ? { ...request, expired: true } : request));
      void commit().finally(() => remove(id));
    }, 5000);
    const request = { id, assets, timer };
    requests.current.set(id, request);
    setPending(current => [...current, request]);
  };
  const undo = (id: number) => {
    const request = requests.current.get(id);
    if (!request || request.expired) return;
    clearTimeout(request.timer);
    remove(id);
  };
  return { pending, hiddenPaths, enqueue, undo };
}
