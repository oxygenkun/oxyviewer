import { useEffect, useRef } from "react";
import { preloadAssetThumbnail } from "../lib/api";
import type { AssetSummary } from "../types";

interface BackgroundPreviewPreloaderProps {
  assets: AssetSummary[];
}

/**
 * Sequentially warms thumbnails which a filter removed from the visible list.
 * Only one lowest-priority request is submitted at a time, keeping the queue
 * available for loupe, visible, and nearby work.
 */
export function BackgroundPreviewPreloader({ assets }: BackgroundPreviewPreloaderProps) {
  const completed = useRef(new Set<string>());
  const candidates = useRef(assets);
  const controller = useRef<AbortController | undefined>(undefined);
  const currentKey = useRef<string | undefined>(undefined);
  const running = useRef(false);
  const mounted = useRef(true);
  const pump = useRef<() => void>(() => undefined);

  pump.current = () => {
    if (!mounted.current || running.current) return;
    const asset = candidates.current.find(
      (candidate) => !completed.current.has(`${candidate.id}:${candidate.modifiedAtMs}`),
    );
    if (!asset) return;

    const key = `${asset.id}:${asset.modifiedAtMs}`;
    const activeController = new AbortController();
    running.current = true;
    currentKey.current = key;
    controller.current = activeController;

    window.setTimeout(() => {
      void preloadAssetThumbnail(asset, activeController.signal)
        .then(() => completed.current.add(key))
        .catch((error) => {
          if (activeController.signal.aborted) return;
          console.warn(`[OxyPreview] background preload failed for ${asset.name}`, error);
          // Do not retry a permanently unreadable file every time another page
          // is appended to the candidate list.
          completed.current.add(key);
        })
        .finally(() => {
          if (controller.current === activeController) controller.current = undefined;
          if (currentKey.current === key) currentKey.current = undefined;
          running.current = false;
          pump.current();
        });
    }, 0);
  };

  useEffect(() => {
    candidates.current = assets;
    const candidateKeys = new Set(
      assets.map((asset) => `${asset.id}:${asset.modifiedAtMs}`),
    );
    if (currentKey.current && !candidateKeys.has(currentKey.current)) {
      controller.current?.abort();
    }
    pump.current();
  }, [assets]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      controller.current?.abort();
    };
  }, []);

  return null;
}
