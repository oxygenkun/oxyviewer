import { generatedPreview, previewUrl, renewMediaResource } from "@/lib/api";
import { preloadBrowserImage } from "@/lib/cache/browserImageCache";
import { retainMediaResource } from "@/lib/cache/mediaResourceLease";
import { imageProjectionKey, useImageProjectionStore } from "@/lib/projection/imageProjection";
import type { AssetSummary, PreviewResult } from "@/types";

/** Wait through RAW's interim camera JPEG before advancing to the next photo. */
function waitForFull(asset: AssetSummary, signal: AbortSignal): Promise<PreviewResult> {
  const sourceRevision = useImageProjectionStore.getState().records[imageProjectionKey(asset.path, "full")]?.sourceRevision;
  return new Promise((resolve, reject) => {
    const cleanup = () => { unsubscribe(); signal.removeEventListener("abort", abort); };
    const abort = () => { cleanup(); reject(signal.reason); };
    const check = () => {
      const record = useImageProjectionStore.getState().records[imageProjectionKey(asset.path, "full")];
      if (!record || record.sourceRevision !== sourceRevision) {
        cleanup();
        reject(new Error("Full decode was invalidated"));
      } else if (record.status === "error") {
        cleanup();
        reject(new Error(record.error ?? "Full decode failed"));
      } else if (record?.result && record.result.satisfaction !== "interim") {
        cleanup();
        resolve(record.result);
      }
    };
    const unsubscribe = useImageProjectionStore.subscribe(check);
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) abort();
    else check();
  });
}

export async function predecodeFullImage(asset: AssetSummary, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  const direct = previewUrl(asset);
  if (direct) return preloadBrowserImage(direct, signal);
  let result = await generatedPreview(asset, "full", signal, "preload");
  if (result?.satisfaction === "interim") result = await waitForFull(asset, signal);
  signal.throwIfAborted();
  if (!result?.url) throw new Error("Full decode returned no image");
  const id = result.resource?.resourceId;
  const release = id ? retainMediaResource(id) : undefined;
  try {
    if (id && !await renewMediaResource(id)) throw new Error("Full image resource expired");
    await preloadBrowserImage(result.url, signal, id);
  } finally {
    release?.();
  }
}
