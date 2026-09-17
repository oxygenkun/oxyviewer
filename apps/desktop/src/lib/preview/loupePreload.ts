import type { AssetSummary } from "@/types";
import { preloadAssetThumbnail } from "@/lib/api";

/**
 * Warms the reusable thumbnail base in both React Query and the WebView image
 * decoder. Full-resolution work remains selection-driven because it can be
 * expensive for RAW and HEIF files. Loupe does not require a separate preview.
 */
export async function preloadAssetLoupeBase(
  asset: AssetSummary,
  rank = 0,
  signal?: AbortSignal,
): Promise<void> {
  const priority = rank === 0 ? "loupe" : "nearby";
  await preloadAssetThumbnail(asset, signal, priority, rank);
}
