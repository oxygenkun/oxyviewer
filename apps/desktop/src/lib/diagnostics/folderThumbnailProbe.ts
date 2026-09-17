import { getMediaResourceStats } from "@/lib/api";
import { invoke } from "@tauri-apps/api/core";
import { getFolderThumbnail, getFolderThumbnailStats } from "@/lib/cache/folderThumbnailCache";
import { perfMark, perfSnapshot } from "./perfProbe";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary, DebugQueueSnapshot } from "@/types";

/** Explicit isolated desktop scenario; default retains the 1,100-file stress case. */
export async function runFolderThumbnailProbe(assets: () => AssetSummary[], signal: AbortSignal, expectedAssets = 1100): Promise<void> {
  const wait = (ms: number) => new Promise<void>((resolve, reject) => {
    signal.throwIfAborted();
    const abort = () => { clearTimeout(timer); reject(signal.reason); };
    const timer = window.setTimeout(() => { signal.removeEventListener("abort", abort); resolve(); }, ms);
    signal.addEventListener("abort", abort, { once: true });
  });
  useWorkspaceStore.setState({ view: "grid", inspectorOpen: false, leftPanelOpen: false });
  const started = performance.now();
  let sampling = true;
  const sampleQueues = async () => {
    while (sampling && !signal.aborted) {
      const requestedAt = performance.now();
      try {
        const snapshot = await invoke<DebugQueueSnapshot>("get_debug_queue_snapshot");
        perfMark("folder-thumbnails:queue-snapshot", {
          elapsedMs: performance.now() - requestedAt,
          workerWaitMicros: snapshot.workerWaitMicros,
          collectionMicros: snapshot.collectionMicros,
          staleQueues: snapshot.staleQueues,
        });
      } catch (error) {
        perfMark("folder-thumbnails:queue-snapshot-error", { message: String(error) });
        return;
      }
      await wait(250);
    }
  };
  const sampler = sampleQueues().catch(() => undefined);
  try {
    for (;;) {
      const files = assets();
      if (files.length >= expectedAssets && files.every((asset) => getFolderThumbnail(asset))) break;
      if (performance.now() - started > 150_000) {
        throw new Error(`Folder warming timed out: ${getFolderThumbnailStats().count}/${files.length}`);
      }
      await wait(100);
    }
  } finally {
    sampling = false;
    await sampler;
  }
  const retained = getFolderThumbnailStats();
  perfMark("folder-thumbnails:warmed", { ...retained, elapsedMs: performance.now() - started });
  const before = perfSnapshot().filter((mark) => mark.name === "preview:queued").length;
  const waits: number[] = [];
  for (const fraction of [1, 0, 0.5, 0]) {
    const scroller = document.querySelector<HTMLElement>(".asset-scroll");
    if (!scroller) throw new Error("Grid scroll surface missing");
    const jumpedAt = performance.now();
    scroller.scrollTop = (scroller.scrollHeight - scroller.clientHeight) * fraction;
    scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
    // Let virtual mounts reflect this destination before checking their pixels.
    await wait(40);
    let ready = false;
    for (let attempt = 0; attempt < 100; attempt++) {
      const viewport = scroller.getBoundingClientRect();
      const cards = [...scroller.querySelectorAll<HTMLElement>(".asset-card")].filter((card) => {
        const bounds = card.getBoundingClientRect();
        return bounds.bottom > viewport.top && bounds.top < viewport.bottom;
      });
      if (cards.length && cards.every((card) => {
        const image = card.querySelector<HTMLImageElement>(".thumbnail img:not(.thumbnail__pending-image)");
        return image?.src.startsWith("blob:") && image.complete && image.naturalWidth > 0;
      })) {
        ready = true;
        waits.push(performance.now() - jumpedAt);
        perfMark("folder-thumbnails:viewport-ready", { fraction, count: cards.length, elapsedMs: waits.at(-1) });
        break;
      }
      await wait(20);
    }
    if (!ready) throw new Error(`Cached grid did not paint at ${fraction}`);
  }
  const after = perfSnapshot().filter((mark) => mark.name === "preview:queued").length;
  if (after !== before) throw new Error(`Warm grid issued ${after - before} native preview requests`);
  const stats = await getMediaResourceStats();
  if (!stats || stats.peakEntries > stats.maxEntries || stats.peakEncodedBytes > stats.maxEncodedBytes) {
    throw new Error("Native resource budget was exceeded");
  }
  if (getFolderThumbnailStats().count !== retained.count || !getFolderThumbnail(assets()[0])) {
    throw new Error("Folder thumbnails were evicted during scrolling");
  }
  perfMark("resource:stress-complete", {
    mode: "folder-thumbnails", ...retained, viewportWaitsMs: waits,
    nativeRequestsOnWarmScroll: after - before, native: stats,
  });
}
