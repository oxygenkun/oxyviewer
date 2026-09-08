import type { AssetSummary } from "../types";
import { generatedPreview, isTauri, previewUrl } from "./api";
import { preloadBrowserImage } from "./browserImageCache";
import { renderPlan } from "./preview";
import { browserPreloadQueue, orderedPriorityWeight } from "./previewQueue";

/**
 * Warms the first useful loupe stage in both React Query and the WebView image
 * decoder. Full-resolution work remains selection-driven because it can be
 * expensive for RAW and HEIF files.
 */
export async function preloadAssetLoupePreview(
  asset: AssetSummary,
  queueOrder = 0,
  signal?: AbortSignal,
): Promise<void> {
  if (!isTauri()) return;

  const previewStep = renderPlan(asset.kind, "loupe")[0];
  const previewMethod = previewStep.method;
  const priority = queueOrder === 0 ? "loupe" : "nearby";
  const preload = (source: string) => browserPreloadQueue.enqueue(
    orderedPriorityWeight(priority, queueOrder),
    signal,
    () => preloadBrowserImage(source, signal),
  );
  if (previewMethod.type === "originalImage") {
    const source = previewUrl(asset);
    if (source) await preload(source);
    return;
  }
  if (previewMethod.type !== "generatedImage") return;

  const result = await generatedPreview(
    asset,
    previewMethod.requestLevel,
    signal,
    priority,
    queueOrder,
  );
  if (result) await preload(result.url);
}
