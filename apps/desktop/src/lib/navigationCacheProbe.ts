import { onPerfMark, perfMark } from "./perfProbe";
import { useWorkspaceStore } from "../store";
import type { AssetSummary } from "../types";

/** Real renderer/IPC round trips, enabled only by the isolated performance harness. */
export async function runNavigationCacheProbe(assets: readonly AssetSummary[], signal: AbortSignal): Promise<void> {
  const pair = assets.slice(0, 2);
  if (pair.length !== 2) throw new Error("Cache navigation requires two distinct assets");
  const wait = (ms: number) => new Promise<void>((resolve, reject) => {
    signal.throwIfAborted();
    const abort = () => { clearTimeout(timer); reject(signal.reason); };
    const timer = window.setTimeout(() => { signal.removeEventListener("abort", abort); resolve(); }, ms);
    signal.addEventListener("abort", abort, { once: true });
  });
  const displayed = (asset: AssetSummary) => {
    if (document.querySelector<HTMLElement>(".loupe__render")?.dataset.assetId !== asset.id) return false;
    if (asset.kind === "heif") return Boolean(document.querySelector(".loupe__raw-status--complete"));
    const image = document.querySelector<HTMLImageElement>(".loupe__render > .thumbnail img:not(.thumbnail__pending-image)");
    return Boolean(image?.complete && image.naturalWidth);
  };
  for (const displaySharpening of pair[0].kind === "heif" ? [false, true] : [false]) {
    useWorkspaceStore.setState({ view: "loupe", displaySharpening, inspectorOpen: false });
    const warmedSources = new Map<string, string>();
    for (const asset of pair) {
      let fullReady = asset.kind !== "heif";
      const stop = onPerfMark((mark) => {
        if (mark.detail?.assetName === asset.name && (
          mark.name === "heif:all-tiles-painted"
          || (mark.name === "image:loaded" && mark.detail?.stage === "full")
        )) fullReady = true;
      });
      try {
        useWorkspaceStore.getState().select(asset.id);
        const deadline = performance.now() + 15_000;
        while ((!displayed(asset) || !fullReady) && performance.now() < deadline) await wait(20);
        if (!displayed(asset) || !fullReady) throw new Error(`Warmup never displayed ${asset.name}`);
        const image = document.querySelector<HTMLImageElement>(".loupe__render > .thumbnail img:not(.thumbnail__pending-image)");
        if (image) warmedSources.set(asset.id, image.src);
      } finally { stop(); }
    }
    for (const asset of [pair[0], pair[1], pair[0], pair[1]]) {
      let cacheHit = false;
      const stop = onPerfMark((mark) => {
        if (mark.name === "heif:memory-cache-hit" && mark.detail?.assetName === asset.name) cacheHit = true;
      });
      const started = performance.now();
      try {
        useWorkspaceStore.getState().select(asset.id);
        await wait(0);
        while (!displayed(asset) && performance.now() - started < 150) await wait(5);
        const elapsedMs = performance.now() - started;
        const image = document.querySelector<HTMLImageElement>(".loupe__render > .thumbnail img:not(.thumbnail__pending-image)");
        const correctSource = asset.kind === "heif" ? cacheHit : image?.src === warmedSources.get(asset.id);
        if (!displayed(asset) || elapsedMs > 150 || !correctSource) {
          throw new Error(`Cached return to ${asset.name} was not immediate (${elapsedMs.toFixed(1)} ms)`);
        }
        perfMark("navigation:cache-return", { assetName: asset.name, elapsedMs, displaySharpening, heifMemoryHit: cacheHit });
        await wait(50);
      } finally { stop(); }
    }
  }
  perfMark("resource:stress-complete", { mode: "navigation-cache" });
}
