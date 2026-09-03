import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { preloadAssetLoupePreview } from "../lib/loupePreload";
import type { AssetSummary } from "../types";

interface FilmstripPreviewPreloaderProps {
  assets: AssetSummary[];
}

/** Sequentially prepares visible filmstrip items in the supplied priority order. */
export function FilmstripPreviewPreloader({ assets }: FilmstripPreviewPreloaderProps) {
  const queryClient = useQueryClient();
  const completed = useRef(new Set<string>());
  const candidates = useRef(assets);
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
    const queueOrder = candidates.current.indexOf(asset);
    running.current = true;
    window.setTimeout(() => {
      if (!mounted.current) {
        running.current = false;
        return;
      }
      void preloadAssetLoupePreview(queryClient, asset, queueOrder)
        .then(() => completed.current.add(key))
        .catch((error) => {
          console.warn(`[OxyPreview] filmstrip preload failed for ${asset.name}`, error);
          completed.current.add(key);
        })
        .finally(() => {
          running.current = false;
          pump.current();
        });
    }, 0);
  };

  useEffect(() => {
    candidates.current = assets;
    pump.current();
  }, [assets]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  return null;
}
