import { useEffect, useRef } from "react";
import { preloadAssetThumbnail } from "@/lib/api";
import type { AssetSummary } from "@/types";
import { folderThumbnailKey, getFolderThumbnail, useFolderThumbnailGeneration } from "@/lib/cache/folderThumbnailCache";

interface BackgroundPreviewPreloaderProps {
  assets: AssetSummary[];
}

/**
 * Sequentially warms the entire directory, including filtered-out files last.
 * Only one lowest-priority request is submitted at a time, keeping the queue
 * available for loupe, visible, and nearby work.
 */
export function BackgroundPreviewPreloader({ assets }: BackgroundPreviewPreloaderProps) {
  const failed = useRef(new Set<string>());
  const generation = useFolderThumbnailGeneration();
  const previousGeneration = useRef(generation);
  const candidates = useRef(assets);
  const cursor = useRef(0);
  const started = useRef(false);
  const controller = useRef<AbortController | undefined>(undefined);
  const currentKey = useRef<string | undefined>(undefined);
  const running = useRef(false);
  const mounted = useRef(true);
  const pump = useRef<() => void>(() => undefined);

  pump.current = () => {
    if (!mounted.current || running.current) return;
    let asset: AssetSummary | undefined;
    while (cursor.current < candidates.current.length) {
      const candidate = candidates.current[cursor.current++];
      if (!getFolderThumbnail(candidate) && !failed.current.has(folderThumbnailKey(candidate))) {
        asset = candidate;
        break;
      }
    }
    if (!asset) return;

    const key = folderThumbnailKey(asset);
    const activeController = new AbortController();
    running.current = true;
    currentKey.current = key;
    controller.current = activeController;

    const delay = started.current ? 0 : 150;
    started.current = true;
    window.setTimeout(() => {
      void Promise.resolve().then(() => {
        activeController.signal.throwIfAborted();
        return preloadAssetThumbnail(asset, activeController.signal);
      })
        .catch((error) => {
          if (activeController.signal.aborted) return;
          console.warn(`[OxyPreview] background preload failed for ${asset.name}`, error);
          // Do not retry a permanently unreadable file every time another page
          // is appended to the candidate list.
          failed.current.add(key);
        })
        .finally(() => {
          if (controller.current === activeController) controller.current = undefined;
          if (currentKey.current === key) currentKey.current = undefined;
          running.current = false;
          pump.current();
        });
    }, delay);
  };

  useEffect(() => {
    candidates.current = assets;
    cursor.current = 0;
    const candidateKeys = new Set(
      assets.map(folderThumbnailKey),
    );
    if (currentKey.current && !candidateKeys.has(currentKey.current)) {
      controller.current?.abort();
    }
    pump.current();
  }, [assets]);

  useEffect(() => {
    if (previousGeneration.current === generation) return;
    previousGeneration.current = generation;
    failed.current.clear();
    cursor.current = 0;
    controller.current?.abort();
    pump.current();
  }, [generation]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      controller.current?.abort();
    };
  }, []);

  return null;
}
