import { useEffect, useRef } from "react";
import { preloadAssetLoupeBase } from "@/lib/preview/loupePreload";
import { protectBrowserImages } from "@/lib/cache/browserImageCache";
import { imageProjectionKey, useImageProjectionStore } from "@/lib/projection/imageProjection";
import { browserImageWorkerCount } from "@/lib/preview/previewQueue";
import { previewUrl } from "@/lib/api";
import type { AssetSummary } from "@/types";

interface FilmstripPreviewPreloaderProps {
  assets: AssetSummary[];
}

/** Reconcile neighbors without restarting requests shared by consecutive viewports. */
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

  const tasks = useRef(new Map<string, { controller: AbortController; done: boolean }>());
  const latestAssets = useRef(assets);
  latestAssets.current = assets;
  const pump = useRef<() => void>(() => {});
  pump.current = () => {
    let running = [...tasks.current.values()].filter((task) => !task.done).length;
    for (const [rank, asset] of latestAssets.current.entries()) {
      if (running >= browserImageWorkerCount) break;
      const key = `${asset.path}\0${asset.modifiedAtMs}`;
      if (tasks.current.has(key)) continue;
      const task = { controller: new AbortController(), done: false };
      tasks.current.set(key, task);
      running += 1;
      void preloadAssetLoupeBase(asset, rank, task.controller.signal)
        .catch(() => undefined)
        .finally(() => {
          task.done = true;
          if (!task.controller.signal.aborted) pump.current();
        });
    }
  };
  useEffect(() => {
    const wanted = new Set(assets.map((asset) => `${asset.path}\0${asset.modifiedAtMs}`));
    for (const [key, task] of tasks.current) {
      if (!wanted.has(key)) {
        task.controller.abort();
        tasks.current.delete(key);
      }
    }
    let disposed = false;
    queueMicrotask(() => { if (!disposed) pump.current(); });
    return () => { disposed = true; };
  }, [assets]);
  useEffect(() => () => {
    for (const task of tasks.current.values()) task.controller.abort();
    tasks.current.clear();
  }, []);

  return null;
}
