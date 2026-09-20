import { addLibraryRoot, cancelFaceAnalysis, getFaceCapability, onLibraryIndexUpdated, startFaceAnalysis } from "@/lib/api";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary } from "@/types";
import { onPerfMark, perfMark } from "./perfProbe";

/** Explicit isolated native probe: real model worker alongside real loupe paints. */
export async function runFaceBrowsingProbe(assets: AssetSummary[], signal: AbortSignal): Promise<void> {
  if (assets.length < 16) throw new Error("Face browsing probe requires at least 16 photos");
  const folder = assets[0].path.replace(/[\\/][^\\/]+$/, "");
  const wait = async () => { signal.throwIfAborted(); await new Promise((resolve) => setTimeout(resolve, 50)); };
  const until = async (ready: () => Promise<boolean>, timeout: number) => {
    const deadline = performance.now() + timeout;
    while (!(await ready())) { if (performance.now() > deadline) throw new Error("Face probe timed out"); await wait(); }
  };
  let indexed = false;
  const stopIndex = await onLibraryIndexUpdated((update) => { if (update.rootPath === folder && update.assetCount >= assets.length) indexed = true; });
  try {
    await addLibraryRoot(folder);
    await until(async () => indexed, 30000);
  } finally { stopIndex(); }
  const jobId = await startFaceAnalysis({ paths: assets.map((asset) => asset.path), force: true });
  try {
    await until(async () => (await getFaceCapability()).stats.analyzedAssets > 0, 60000);
    for (const asset of assets.slice(-8)) {
      if (!(await getFaceCapability()).running) throw new Error("Analysis ended before browsing measurement");
      let loaded = false;
      const stop = onPerfMark((mark) => { if (mark.name === "image:loaded" && mark.detail?.large && mark.detail.assetName === asset.name) loaded = true; });
      const start = performance.now();
      try {
        useWorkspaceStore.getState().select(asset.id);
        useWorkspaceStore.getState().setView("loupe");
        await until(async () => loaded, 10000);
        const elapsedMs = performance.now() - start;
        perfMark("faces:loupe-painted", { assetName: asset.name, elapsedMs });
        if (elapsedMs > 800) throw new Error(`Loupe exceeded cold preview budget: ${elapsedMs.toFixed(0)} ms`);
      } finally { stop(); }
    }
  } finally {
    const start = performance.now();
    await cancelFaceAnalysis(jobId);
    await until(async () => !(await getFaceCapability()).running, 5000);
    perfMark("faces:cancelled", { elapsedMs: performance.now() - start });
  }
  perfMark("resource:stress-complete");
}
