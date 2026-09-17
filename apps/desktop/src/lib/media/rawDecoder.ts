import type { QueryClient } from "@tanstack/react-query";
import type { AssetSummary } from "@/types";
import { retryRawFull } from "@/lib/api";
import { invalidateImageProjection } from "@/lib/projection/imageProjection";
import { assetRenderQueryKey } from "@/lib/preview/preview";

/** Retry the ordered RAW fallback chain without clearing retained thumbnails. */
export async function regenerateRawFull(asset: AssetSummary, client: QueryClient) {
  await retryRawFull(asset.path);
  invalidateImageProjection(asset.path, "full");
  await client.invalidateQueries({
    queryKey: assetRenderQueryKey(asset, { type: "generatedImage", requestLevel: "full" }),
    exact: true,
  });
  await client.invalidateQueries({ queryKey: ["raw-decoder"] });
}
