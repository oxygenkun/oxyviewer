import { useEffect, useRef } from "react";
import { preloadAssetLoupePreview } from "../lib/loupePreload";
import type { AssetSummary } from "../types";

interface FilmstripPreviewPreloaderProps {
  assets: AssetSummary[];
}

/** Sequentially prepares visible filmstrip items in the supplied priority order. */
export function FilmstripPreviewPreloader({ assets }: FilmstripPreviewPreloaderProps) {
  const completed = useRef(new Set<string>());
  const candidates = useRef(assets);
  const running = useRef(false);
  const runningKey = useRef<string | undefined>(undefined);
  const controller = useRef<AbortController | undefined>(undefined);
  const mounted = useRef(true);
  const pump = useRef<() => void>(() => undefined);

  pump.current = () => {
    if (!mounted.current || running.current) return;
    const asset = candidates.current.find(
      (candidate) => !completed.current.has(`${candidate.id}:${candidate.modifiedAtMs}`),
    );
    if (!asset) return;

    const key = `${asset.id}:${asset.modifiedAtMs}`;
    const queueOrder = candidates.current.indexOf(asset);
    running.current = true;
    runningKey.current = key;
    controller.current = new AbortController();
    window.setTimeout(() => {
      if (!mounted.current) {
        running.current = false;
        return;
      }
      const signal = controller.current?.signal;
      void preloadAssetLoupePreview(asset, queueOrder, signal)
        .then(() => completed.current.add(key))
        .catch((error) => {
          if (signal?.aborted) return;
          console.warn(`[OxyPreview] filmstrip preload failed for ${asset.name}`, error);
          completed.current.add(key);
        })
        .finally(() => {
          running.current = false;
          runningKey.current = undefined;
          controller.current = undefined;
          pump.current();
        });
    }, 0);
  };

  useEffect(() => {
    candidates.current = assets;
    if (
      runningKey.current
      && !assets.some((asset) => `${asset.id}:${asset.modifiedAtMs}` === runningKey.current)
    ) {
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
