import { useEffect, useRef } from "react";
import { requestMetadata } from "@/lib/api";
import { acceptMetadataProjections } from "@/lib/projection/metadataProjection";
import type { AssetSummary } from "@/types";

/** Warm markings and tags from discovered sidecars after the first paint. */
export function useSidecarMetadata(scopeKey: string, assets: readonly AssetSummary[]): void {
  const requested = useRef({ scopeKey, paths: new Set<string>() });
  useEffect(() => {
    if (requested.current.scopeKey !== scopeKey) requested.current = { scopeKey, paths: new Set() };
    const seen = requested.current.paths;
    const paths = assets.filter((asset) => asset.hasSidecar && !seen.has(asset.path)).map((asset) => asset.path);
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let offset = 0;
    const nextBatch = async () => {
      const batch = paths.slice(offset, offset + 32).filter((path) => !seen.has(path));
      offset += 32;
      for (const path of batch) seen.add(path);
      try {
        if (batch.length) acceptMetadataProjections(await requestMetadata(batch, "background"));
      } catch {
        for (const path of batch) seen.delete(path);
        return;
      }
      if (!cancelled && offset < paths.length) timer = setTimeout(() => { void nextBatch(); }, 200);
    };
    if (paths.length) timer = setTimeout(() => { void nextBatch(); }, 200);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [scopeKey, assets]);
}
