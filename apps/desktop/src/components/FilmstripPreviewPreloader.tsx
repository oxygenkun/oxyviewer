import { useEffect } from "react";
import { preloadAssetLoupePreview } from "../lib/loupePreload";
import { protectBrowserImages } from "../lib/browserImageCache";
import { imageProjectionKey, useImageProjectionStore } from "../lib/imageProjection";
import { previewUrl } from "../lib/api";
import type { AssetSummary } from "../types";

interface FilmstripPreviewPreloaderProps {
  assets: AssetSummary[];
}

/** Two bounded workers keep a slow NAS read from blocking every neighbor. */
export function FilmstripPreviewPreloader({ assets }: FilmstripPreviewPreloaderProps) {
  const records = useImageProjectionStore((state) => state.records);
  useEffect(() => {
    protectBrowserImages(assets.slice(0, 5).flatMap((asset) => [
      previewUrl(asset),
      ...(["thumbnail", "preview", "full"] as const).map(
        (level) => records[imageProjectionKey(asset.path, level)]?.result?.url,
      ),
    ].filter((url): url is string => Boolean(url))));
    return () => protectBrowserImages([]);
  }, [assets, records]);

  useEffect(() => {
    const controller = new AbortController();
    let next = 0;
    const run = async () => {
      while (!controller.signal.aborted && next < assets.length) {
        const rank = next++;
        const asset = assets[rank];
        try {
          await preloadAssetLoupePreview(asset, rank, controller.signal);
        } catch (error) {
          if (controller.signal.aborted) return;
          console.warn(`[OxyPreview] filmstrip preload failed for ${asset.name}`, error);
        }
      }
    };
    // Defer dispatch so StrictMode's discarded mount cannot start I/O.
    queueMicrotask(() => { void run(); void run(); });
    return () => controller.abort();
  }, [assets]);

  return null;
}
