import { clearPreviewCache, getMediaResourceStats, updateCacheSettings } from "./api";
import { clearImageProjections } from "./imageProjection";
import { onPerfMark, perfMark, perfSnapshot } from "./perfProbe";
import { useWorkspaceStore } from "../store";
import { runNavigationCacheProbe } from "./navigationCacheProbe";
import { runGridScrollProbe } from "./gridScrollProbe";
import { runFolderThumbnailProbe } from "./folderThumbnailProbe";
import type { AssetSummary, PerfScenario } from "../types";

/** Runs only in an explicitly injected, isolated native performance scenario. */
export async function runResourceStress(
  mode: NonNullable<PerfScenario["resourceStress"]>,
  assets: () => AssetSummary[],
  signal: AbortSignal,
  expectedAssets?: number,
): Promise<void> {
  if (mode === "folder-thumbnails") return runFolderThumbnailProbe(assets, signal, expectedAssets);
  if (mode === "grid-scroll") return runGridScrollProbe(signal);
  if (mode === "filmstrip-scroll") return runGridScrollProbe(signal, true);
  if (mode === "navigation-cache") return runNavigationCacheProbe(assets(), signal);
  const wait = (ms: number) => new Promise<void>((resolve, reject) => {
    signal.throwIfAborted();
    const abort = () => { clearTimeout(timer); reject(signal.reason); };
    const timer = window.setTimeout(() => { signal.removeEventListener("abort", abort); resolve(); }, ms);
    signal.addEventListener("abort", abort, { once: true });
  });
  const snapshot = async (stage: string) => {
    const stats = await getMediaResourceStats();
    if (!stats) throw new Error("Native registry diagnostics unavailable");
    perfMark("resource:stats", { stage, ...stats });
    if (stats.peakEntries > stats.maxEntries || stats.peakEncodedBytes > stats.maxEncodedBytes) {
      throw new Error("Registry exceeded its configured budget");
    }
    return stats;
  };
  useWorkspaceStore.setState({
    view: mode, thumbnailOrientation: "portrait", inspectorOpen: false,
    leftPanelOpen: false, displaySharpening: false,
  });
  await wait(500);
  if (mode === "loupe") {
    const batch = assets().slice(0, 80);
    if (batch.length < 80) throw new Error("HIF pressure requires 80 distinct source paths");
    for (const asset of batch) {
      useWorkspaceStore.getState().select(asset.id);
      await wait(120);
    }
    // Revisit distinct images and require actual Full paint, including upgrades
    // that lost their first consumer during the rapid selection pass.
    for (const asset of batch.slice(-8)) {
      let loaded = false;
      const stop = onPerfMark((mark) => {
        if (mark.detail?.assetName !== asset.name) return;
        if (mark.name === "image:loaded" && mark.detail.stage === "full") loaded = true;
        if (mark.name === "heif:all-tiles-painted" && mark.detail.failed === 0
          && Number(mark.detail.drawn) > 0 && mark.detail.drawn === mark.detail.expected) loaded = true;
      });
      const selectedAt = performance.now();
      try {
        perfMark("resource:full-selection", { assetName: asset.name });
        useWorkspaceStore.getState().select(asset.id);
        for (let tries = 0; !loaded && tries < 300; tries += 1) await wait(100);
        if (!loaded) throw new Error(`Full did not paint for ${asset.name}`);
        perfMark("resource:full-selection-painted", { assetName: asset.name, elapsedMs: performance.now() - selectedAt });
      } finally { stop(); }
      await snapshot(asset.name);
    }
  } else {
    let atEnd = 0;
    for (let step = 0; step < 1000 && atEnd < 3; step += 1) {
      const scroller = document.querySelector<HTMLElement>(".asset-scroll");
      if (!scroller) throw new Error("Asset scroll surface missing");
      const previous = scroller.scrollTop;
      scroller.scrollTop += scroller.clientHeight * 0.75;
      scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
      atEnd = scroller.scrollTop === previous ? atEnd + 1 : 0;
      // This older pressure scenario measures bounded resources at a steady
      // pace. Continuous fast scrolling uses the separate grid-scroll probe.
      await wait(350);
      if (step % 10 === 0) await snapshot(`scroll-${step}`);
    }
    const painted = new Set(perfSnapshot().filter((mark) => mark.name === "image:loaded")
      .map((mark) => mark.detail?.assetName));
    if (painted.size < 500) throw new Error(`Only ${painted.size} distinct photos painted`);
    perfMark("resource:scroll-painted", { count: painted.size, mode });
  }
  // Start real protocol reads while clear and prune execute through their
  // production commands. The runner supplies isolated data/cache directories.
  const urls = [...new Set(Array.from(document.querySelectorAll<HTMLImageElement>("img"))
    .map((image) => image.src).filter((url) => url.includes("/resource/")))].slice(0, 4);
  if (!urls.length) throw new Error("No displayed media protocol resources");
  const read = async (url: string, phase: string) => {
    const response = await fetch(url, { signal });
    if (!response.ok || (await response.arrayBuffer()).byteLength === 0) {
      const mounted = Array.from(document.querySelectorAll<HTMLImageElement>("img")).filter((image) => image.src === url);
      throw new Error(`Active resource could not be read ${phase}: ${response.status} ${url}; mounted=${mounted.length}; classes=${mounted.map((image) => image.className).join(",")}`);
    }
  };
  await Promise.all(urls.map((url) => read(url, "before cache maintenance")));
  await Promise.all([...urls.map((url) => read(url, "during cache clear")), clearPreviewCache()]);
  // Match SettingsPanel: an explicit cache clear drops the UI cache too.
  // Mounted presentations retain their own leases while cached-only pins leave.
  clearImageProjections();
  await Promise.all([...urls.map((url) => read(url, "after cache clear")), updateCacheSettings(null, 1024 ** 3)]);
  perfMark("resource:maintenance-read", { count: urls.length });
  await wait(11_000); // publish grace expires and at least one UI heartbeat runs
  const stats = await snapshot("settled");
  const displayed = document.querySelectorAll("img[src*='/resource/']").length;
  if (stats.entries > displayed + 16 || stats.uiLeased > displayed + 16) {
    throw new Error(`Registry did not settle near the displayed/pending set (${displayed})`);
  }
  perfMark("resource:stress-complete", { mode, displayed, ...stats });
}
