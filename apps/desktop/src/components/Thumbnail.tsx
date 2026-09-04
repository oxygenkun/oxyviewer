import { useQuery } from "@tanstack/react-query";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  generatedPreview,
  isTauri,
  previewUrl,
} from "../lib/api";
import {
  browserImageSourceWhenEnabled,
  getBrowserImageSize,
  isBrowserImageReady,
  markBrowserImageReady,
} from "../lib/browserImageCache";
import { perfMark } from "../lib/perfProbe";
import { imageProjectionKey, useImageProjectionStore } from "../lib/imageProjection";
import {
  assetRenderQueryKey,
  loupeThumbnailFallback,
  renderMethodKey,
  renderPlan,
  type RenderMethod,
} from "../lib/preview";
import { beginPreviewDebug, type PreviewDebugHandle } from "../lib/previewDebug";
import { nextProgressiveStage } from "../lib/progressiveImage";
import { rawPreviewStatus, type RawPreviewStatus } from "../lib/rawPreview";
import type {
  AssetSummary,
  PreviewPriority,
  PreviewResult,
  RenderLevel,
} from "../types";

interface ThumbnailProps {
  asset: AssetSummary;
  enabled?: boolean;
  large?: boolean;
  priority?: PreviewPriority;
  queueOrder?: number;
  onContextMenu?: React.MouseEventHandler<HTMLDivElement>;
  onImageLoad?: (size: { width: number; height: number }) => void;
  onRawPreviewStatus?: (status: RawPreviewStatus) => void;
}

interface DisplayedImage {
  assetId: string;
  source: string;
}

function hashSeed(value: string) {
  let seed = 0;
  for (let index = 0; index < value.length; index += 1) {
    seed = (seed * 31 + value.charCodeAt(index)) % 360;
  }
  return seed;
}

function generatedLevel(method: RenderMethod | undefined): RenderLevel | undefined {
  return method?.type === "generatedImage" ? method.requestLevel : undefined;
}

export function Thumbnail({
  asset,
  enabled = true,
  large = false,
  priority = "visible",
  queueOrder = 0,
  onContextMenu,
  onImageLoad,
  onRawPreviewStatus,
}: ThumbnailProps) {
  const [failed, setFailed] = useState(false);
  const [fullImageFailed, setFullImageFailed] = useState(false);
  const [loaded, setLoaded] = useState<{ assetId: string; mode: "preview" | "full" }>();
  const [displayedImage, setDisplayedImage] = useState<DisplayedImage>();
  const imageDebug = useRef<{ source: string; handle: PreviewDebugHandle } | undefined>(undefined);
  const plan = useMemo(
    () => renderPlan(asset.kind, large ? "loupe" : "thumbnail"),
    [asset.kind, large],
  );
  const previewStep = plan[0];
  const fullStep = plan.find((step) => step.level === "full");
  const previewMethod = previewStep.method;
  const fullMethod = fullStep?.method;
  const fallbackThumbnailStep = useMemo(
    () => large ? loupeThumbnailFallback(asset.kind) : undefined,
    [asset.kind, large],
  );
  const previewLevel = generatedLevel(previewMethod);
  const fullLevel = generatedLevel(fullMethod);
  const previewMethodIdentity = renderMethodKey(previewMethod);
  const fallbackThumbnailLevel = generatedLevel(fallbackThumbnailStep?.method);
  const fullMethodIdentity = fullMethod ? renderMethodKey(fullMethod) : undefined;
  const distinctFullLevel = fullMethodIdentity !== previewMethodIdentity ? fullLevel : undefined;
  const thumbnailProjection = useImageProjectionStore((state) => fallbackThumbnailLevel
    ? state.records[imageProjectionKey(asset.path, fallbackThumbnailLevel)]
    : undefined);
  const previewProjection = useImageProjectionStore((state) => previewLevel
    ? state.records[imageProjectionKey(asset.path, previewLevel)]
    : undefined);
  const fullProjection = useImageProjectionStore((state) => distinctFullLevel
    ? state.records[imageProjectionKey(asset.path, distinctFullLevel)]
    : undefined);
  const ownsFullDetailStage = asset.kind === "raw"
    || (asset.kind === "heif" && fullMethod?.type === "generatedImage");
  const directSource = useMemo(
    () => previewMethod.type === "originalImage" ? previewUrl(asset) : undefined,
    [asset, previewMethod.type],
  );
  const requestPriority = large ? "loupe" : priority;
  const previewQuery = useQuery({
    queryKey: assetRenderQueryKey(asset, previewMethod),
    queryFn: async ({ signal }) => {
      await generatedPreview(
        asset,
        previewLevel ?? previewStep.level,
        signal,
        requestPriority,
        queueOrder,
      );
    },
    enabled: enabled && isTauri() && Boolean(previewLevel),
    staleTime: Infinity,
    retry: 0,
  });
  const fullQuery = useQuery({
    queryKey: fullMethod
      ? assetRenderQueryKey(asset, fullMethod)
      : ["asset-render", asset.id, asset.modifiedAtMs, "no-full-image"],
    queryFn: async ({ signal }) => {
      await generatedPreview(
        asset,
        distinctFullLevel ?? "full",
        signal,
        "loupe",
        queueOrder,
      );
    },
    enabled: enabled
      && isTauri()
      && large
      && Boolean(distinctFullLevel)
      && Boolean(previewProjection?.result || previewQuery.isError || directSource),
    staleTime: Infinity,
    retry: 0,
  });
  const thumbnailSource = thumbnailProjection?.result;
  const previewSource = previewProjection?.result;
  const fullSource = fullProjection?.result;
  const preparedSource = directSource && isBrowserImageReady(directSource)
    ? directSource
    : previewSource && isBrowserImageReady(previewSource.url)
      ? previewSource.url
      : thumbnailSource && isBrowserImageReady(thumbnailSource.url)
        ? thumbnailSource.url
      : undefined;
  const preparedSize = preparedSource ? getBrowserImageSize(preparedSource) : undefined;
  const visibleImage = displayedImage?.assetId === asset.id
    ? displayedImage
    : preparedSource
      ? { assetId: asset.id, source: preparedSource }
      : undefined;
  const generatedSource = nextProgressiveStage(Boolean(visibleImage), [
    thumbnailSource,
    previewSource,
    !fullImageFailed ? fullSource : undefined,
  ]);
  const sourceCandidate = directSource ?? generatedSource?.url;
  // A mounted virtual row must not begin filesystem I/O or image decoding
  // while its scroll container is moving. Already-decoded browser images can
  // still paint immediately from the local in-memory cache.
  const source = browserImageSourceWhenEnabled(sourceCandidate, enabled);
  const pendingSource = source !== visibleImage?.source ? source : undefined;
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
            priority: requestPriority,
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
    imageDebug.current?.handle.updatePriority(requestPriority);
  }, [requestPriority]);

  useEffect(() => {
    setLoaded(undefined);
    setDisplayedImage(undefined);
    setFullImageFailed(false);
  }, [asset.id]);

  useLayoutEffect(() => {
    if (!large || !preparedSize) return;
    onImageLoad?.(preparedSize);
    if (!ownsFullDetailStage || previewSource?.url !== preparedSource) return;
    setLoaded((current) => current?.assetId === asset.id
      ? current
      : { assetId: asset.id, mode: "preview" });
  }, [
    asset.id,
    asset.kind,
    large,
    onImageLoad,
    ownsFullDetailStage,
    preparedSize,
    preparedSource,
    previewSource?.url,
  ]);

  useEffect(() => {
    // The RAW full-detail endpoint can deliberately reuse the already-loaded
    // embedded JPEG. No second image load event fires when the path is equal.
    if (
      loaded?.assetId === asset.id
      && loaded.mode === "preview"
      && fullSource?.path
      && fullSource.path === previewSource?.path
    ) {
      setLoaded({ assetId: asset.id, mode: "full" });
    }
  }, [asset.id, fullSource?.path, loaded, previewSource?.path]);

  useEffect(() => {
    // Windows/Linux HEIF status is owned by the tile canvas. macOS HEIF and
    // RAW own a file-backed full stage here.
    if (!onRawPreviewStatus || !large || !ownsFullDetailStage) return;
    onRawPreviewStatus(rawPreviewStatus({
      assetId: asset.id,
      loaded,
      fullError: fullQuery.isError || fullProjection?.status === "error" || fullImageFailed,
      fullSize: fullSource,
    }));
  }, [
    asset.id,
    asset.kind,
    fullSource?.height,
    fullSource?.width,
    fullProjection?.status,
    fullQuery.isError,
    fullImageFailed,
    large,
    loaded,
    onRawPreviewStatus,
    ownsFullDetailStage,
  ]);

  const handleLoad = (size: { width: number; height: number }, result?: PreviewResult) => {
    const loadedLevel = result && result === fullSource
      ? fullStep?.level ?? "full"
      : result && result === thumbnailSource
        ? "thumbnail"
      : previewStep.level;
    perfMark("image:loaded", {
      assetName: asset.name,
      large,
      stage: directSource ? "direct" : loadedLevel,
      renderLevel: loadedLevel,
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
            priority: requestPriority,
          })
        : undefined;
      debug?.start();
      if (debug) imageDebug.current = { source, handle: debug };
    }
    debug?.complete();
    if (source) markBrowserImageReady(source, size);
    if (ownsFullDetailStage && large && result && result !== thumbnailSource) {
      setLoaded({ assetId: asset.id, mode: result === fullSource ? "full" : "preview" });
    }
    if (source) setDisplayedImage({ assetId: asset.id, source });
    onImageLoad?.(size);
  };

  const handleError = (result?: PreviewResult) => {
    perfMark("image:error", {
      assetName: asset.name,
      large,
      stage: directSource ? "direct" : (result?.renderLevel ?? previewStep.level),
    });
    const currentDebug = imageDebug.current;
    let debug: PreviewDebugHandle | undefined;
    if (currentDebug && currentDebug.source === source) debug = currentDebug.handle;
    if (!debug && source) {
      debug = __OXY_DEBUG__
        ? beginPreviewDebug({
            assetName: asset.name,
            stage: directSource ? "image-direct" : "image-decode",
            priority: requestPriority,
          })
        : undefined;
      debug?.start();
      if (debug) imageDebug.current = { source, handle: debug };
    }
    debug?.fail();
    if (result && result === fullSource) {
      setFullImageFailed(true);
    } else {
      setFailed(true);
    }
  };

  return (
    <div
      className={`thumbnail ${large ? "thumbnail--large" : ""}`}
      style={style}
      onContextMenu={onContextMenu}
    >
      {!visibleImage ? (
        <div className="thumbnail__fallback" aria-hidden="true">
          <span>{asset.extension}</span>
          <i />
        </div>
      ) : null}
      {visibleImage ? (
        <img
          key={`${asset.id}:${visibleImage.source}`}
          src={visibleImage.source}
          alt=""
          draggable={false}
        />
      ) : null}
      {pendingSource && !failed ? (
        <img
          className="thumbnail__pending-image"
          key={`${asset.id}:${pendingSource}`}
          src={pendingSource}
          alt=""
          draggable={false}
          onError={() => handleError(generatedSource)}
          onLoad={(event) => handleLoad({
            width: event.currentTarget.naturalWidth,
            height: event.currentTarget.naturalHeight,
          }, generatedSource)}
        />
      ) : null}
      {asset.kind === "raw" ? <span className="thumbnail__badge">RAW</span> : null}
    </div>
  );
}
