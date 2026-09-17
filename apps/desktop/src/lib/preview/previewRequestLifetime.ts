// React Query can settle at Interim while Rust is still upgrading the image.
// Track the same shared render identity until the last UI observer leaves.
const lifetimes = new Map<string, { owners: number; controller: AbortController }>();

function lifetime(key: string) {
  let entry = lifetimes.get(key);
  if (!entry) {
    entry = { owners: 0, controller: new AbortController() };
    lifetimes.set(key, entry);
  }
  return entry;
}

export function retainPreviewRequest(key: string): () => void {
  const entry = lifetime(key);
  entry.owners += 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    entry.owners -= 1;
    queueMicrotask(() => {
      if (entry.owners || lifetimes.get(key) !== entry) return;
      lifetimes.delete(key);
      entry.controller.abort();
    });
  };
}

export function previewRequestSignal(key: string, querySignal: AbortSignal): AbortSignal {
  return AbortSignal.any([querySignal, lifetime(key).controller.signal]);
}
