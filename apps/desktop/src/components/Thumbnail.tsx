import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { generatedPreview, isTauri, previewUrl } from "../lib/api";
import { perfMark } from "../lib/perfProbe";
import { previewStages } from "../lib/preview";
import { beginPreviewDebug, type PreviewDebugHandle } from "../lib/previewDebug";
import { rawPreviewStatus, type RawPreviewStatus } from "../lib/rawPreview";
import type { AssetKind, AssetSummary, PreviewPriority, PreviewResult } from "../types";

interface ThumbnailProps {
  asset: AssetSummary;
  enabled?: boolean;
  large?: boolean;
  priority?: PreviewPriority;
  onImageLoad?: (size: { width: number; height: number }) => void;
  onRawPreviewStatus?: (status: RawPreviewStatus) => void;
}

function hashSeed(value: string) {
  let seed = 0;
  for (let index = 0; index < value.length; index += 1) {
    seed = (seed * 31 + value.charCodeAt(index)) % 360;
  }
  return seed;
}

/**
 * Formats whose loupe experience includes a full-resolution single-image stage
 * (developed by the backend into one JPEG). HEIF is intentionally excluded:
 * its full resolution is streamed as tiles by `HeifTileCanvas`, so the
 * progressive `<img>` chain keeps only a 512px placeholder for HEIF.
 */
function hasFullDetailStage(kind: AssetKind): boolean {
  return kind === "raw";
}

export function Thumbnail({
  asset,
  enabled = true,
  large = false,
  priority = "visible",
  onImageLoad,
  onRawPreviewStatus,
}: ThumbnailProps) {
  const [failed, setFailed] = useState(false);
  const [fullImageFailed, setFullImageFailed] = useState(false);
  const [loaded, setLoaded] = useState<{ assetId: string; mode: "preview" | "full" }>();
  const imageDebug = useRef<{ source: string; handle: PreviewDebugHandle } | undefined>(undefined);
  const directSource = useMemo(() => previewUrl(asset), [asset]);
  const stages = previewStages(asset.kind, large);
  // stages[0] is always the numeric thumbnail size (512) per previewStages.
  const thumbnailSize = stages[0] as number;
  const loupeSize = stages.length > 1 && stages[1] !== "full" ? (stages[1] as number) : undefined;
  const hasFullStage = stages.includes("full");
  const thumbnailPriority = large ? "loupe" : priority;
  const thumbnailSource = useQuery({
    queryKey: ["asset-preview", asset.id, asset.modifiedAtMs, thumbnailSize, thumbnailPriority],
    queryFn: ({ signal }) => generatedPreview(
      asset,
      large ? "loupePreview" : "thumbnail",
      thumbnailSize,
      signal,
      thumbnailPriority,
    ),
    enabled: enabled && isTauri() && !directSource,
    staleTime: Infinity,
    retry: 0,
  });
  const loupeSource = useQuery({
    queryKey: ["asset-preview", asset.id, asset.modifiedAtMs, loupeSize ?? "loupe"],
    queryFn: ({ signal }) => generatedPreview(
      asset,
      "loupePreview",
      loupeSize ?? thumbnailSize,
      signal,
      "loupe",
    ),
    enabled: enabled && isTauri() && !directSource && Boolean(loupeSize && thumbnailSource.data),
    staleTime: Infinity,
    retry: 0,
  });
  const fullSource = useQuery({
    queryKey: ["asset-preview", asset.id, asset.modifiedAtMs, "fullDetail"],
    queryFn: ({ signal }) => generatedPreview(asset, "fullDetail", undefined, signal, "loupe"),
    enabled: enabled
      && isTauri()
      && hasFullStage
      && large
      && Boolean(loupeSize
        ? loupeSource.data || loupeSource.isError
        : thumbnailSource.data || thumbnailSource.isError),
    staleTime: Infinity,
    retry: 0,
  });
  const previewSource = loupeSource.data ?? thumbnailSource.data;
  const generatedSource = (!fullImageFailed ? fullSource.data : undefined) ?? previewSource;
  const source = directSource ?? generatedSource?.url;
  const seed = hashSeed(asset.name);
  const style = {
    "--thumb-hue": `${seed}`,
    "--thumb-hue-two": `${(seed + 72) % 360}`,
  } as React.CSSProperties;

  useEffect(() => setFailed(false), [source]);
  useEffect(() => {
    if (!source || failed) return;
    let disposed = false;

    // Deferring one microtask suppresses React StrictMode's throwaway effect
    // cycle, keeping development timing output one-to-one with real loads.
    queueMicrotask(() => {
      if (disposed || imageDebug.current?.source === source) return;
      const handle = __OXY_DEBUG__
        ? beginPreviewDebug({
            assetName: asset.name,
            stage: directSource ? "image-direct" : "image-decode",
            priority: thumbnailPriority,
          })
        : undefined;
      if (!handle) return;
      imageDebug.current = { source, handle };
      handle.start();
    });

    return () => {
      disposed = true;
      if (imageDebug.current?.source !== source) return;
      imageDebug.current.handle.cancel();
      imageDebug.current = undefined;
    };
  }, [asset.name, directSource, failed, source]);

  useEffect(() => {
    imageDebug.current?.handle.updatePriority(thumbnailPriority);
  }, [thumbnailPriority]);

  useEffect(() => {
    setLoaded(undefined);
    setFullImageFailed(false);
  }, [asset.id]);

  useEffect(() => {
    // The RAW full-detail endpoint can deliberately reuse the already-loaded
    // embedded JPEG. No second image load event fires when the path is equal.
    if (
      loaded?.assetId === asset.id
      && loaded.mode === "preview"
      && fullSource.data?.path
      && fullSource.data.path === previewSource?.path
    ) {
      setLoaded({ assetId: asset.id, mode: "full" });
    }
  }, [asset.id, fullSource.data?.path, loaded, previewSource?.path]);

  useEffect(() => {
    // Report progressive status only for formats that run the full-detail
    // stage here (currently RAW). HEIF's full-resolution status is owned by
    // the tile canvas and surfaced through a separate event channel.
    if (!onRawPreviewStatus || !large || !hasFullDetailStage(asset.kind)) return;
    onRawPreviewStatus(rawPreviewStatus({
      assetId: asset.id,
      loaded,
      fullError: fullSource.isError || fullImageFailed,
      fullSize: fullSource.data,
    }));
  }, [
    asset.id,
    asset.kind,
    fullSource.data?.height,
    fullSource.data?.width,
    fullSource.isError,
    fullImageFailed,
    large,
    loaded,
    onRawPreviewStatus,
  ]);

  const handleLoad = (size: { width: number; height: number }, result?: PreviewResult) => {
    // Classify by which progressive query produced the result; the backend
    // `stage` field is absent for cache hits and direct passthrough.
    const probeStage = directSource
      ? "direct"
      : result && result === fullSource.data
        ? "full"
        : result && result === loupeSource.data
          ? "loupe4096"
          : "thumb512";
    perfMark("image:loaded", {
      assetName: asset.name,
      large,
      stage: probeStage,
      mode: result === fullSource.data ? "full" : "preview",
      width: size.width,
      height: size.height,
    });
    const currentDebug = imageDebug.current;
    let debug: PreviewDebugHandle | undefined;
    if (currentDebug && currentDebug.source === source) debug = currentDebug.handle;
    if (!debug && source) {
      // A memory-cached image can finish before the effect above runs.
      debug = __OXY_DEBUG__
        ? beginPreviewDebug({
            assetName: asset.name,
            stage: directSource ? "image-direct" : "image-decode",
            priority: thumbnailPriority,
          })
        : undefined;
      debug?.start();
      if (debug) imageDebug.current = { source, handle: debug };
    }
    debug?.complete();
    if (hasFullDetailStage(asset.kind) && large && result) {
      setLoaded({ assetId: asset.id, mode: result === fullSource.data ? "full" : "preview" });
    }
    onImageLoad?.(size);
  };

  const handleError = (result?: PreviewResult) => {
    perfMark("image:error", {
      assetName: asset.name,
      large,
      stage: directSource ? "direct" : (result?.stage ?? "generated"),
    });
    const currentDebug = imageDebug.current;
    let debug: PreviewDebugHandle | undefined;
    if (currentDebug && currentDebug.source === source) debug = currentDebug.handle;
    if (!debug && source) {
      debug = __OXY_DEBUG__
        ? beginPreviewDebug({
            assetName: asset.name,
            stage: directSource ? "image-direct" : "image-decode",
            priority: thumbnailPriority,
          })
        : undefined;
      debug?.start();
      if (debug) imageDebug.current = { source, handle: debug };
    }
    debug?.fail();
    if (result && result === fullSource.data) {
      setFullImageFailed(true);
    } else {
      setFailed(true);
    }
  };

  return (
    <div className={`thumbnail ${large ? "thumbnail--large" : ""}`} style={style}>
      {source && !failed ? (
        <img
          key={`${asset.id}:${source}`}
          src={source}
          alt=""
          draggable={false}
          onError={() => handleError(generatedSource)}
          onLoad={(event) => handleLoad({
            width: event.currentTarget.naturalWidth,
            height: event.currentTarget.naturalHeight,
          }, generatedSource)}
        />
      ) : (
        <div className="thumbnail__fallback" aria-hidden="true">
          <span>{asset.extension}</span>
          <i />
        </div>
      )}
      {asset.kind === "raw" ? <span className="thumbnail__badge">RAW</span> : null}
    </div>
  );
}
