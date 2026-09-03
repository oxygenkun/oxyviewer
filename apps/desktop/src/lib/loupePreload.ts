import type { QueryClient } from "@tanstack/react-query";
import type { AssetSummary } from "../types";
import { generatedPreview, isTauri, previewUrl } from "./api";
import { preloadBrowserImage } from "./browserImageCache";
import { assetRenderQueryKey, renderPlan } from "./preview";

/**
 * Warms the first useful loupe stage in both React Query and the WebView image
 * decoder. Full-resolution work remains selection-driven because it can be
 * expensive for RAW and HEIF files.
 */
export async function preloadAssetLoupePreview(
  queryClient: QueryClient,
  asset: AssetSummary,
  queueOrder = 0,
): Promise<void> {
  if (!isTauri()) return;

  const previewStep = renderPlan(asset.kind, "loupe")[0];
  const previewMethod = previewStep.method;
  if (previewMethod.type === "originalImage") {
    const source = previewUrl(asset);
    if (source) await preloadBrowserImage(source);
    return;
  }

  const result = await queryClient.fetchQuery({
    queryKey: assetRenderQueryKey(asset, previewMethod),
    queryFn: ({ signal }) => generatedPreview(
      asset,
      previewMethod.requestLevel,
      signal,
      "visible",
      queueOrder,
    ),
    staleTime: Infinity,
    retry: 0,
  });
  if (result) await preloadBrowserImage(result.url);
}
