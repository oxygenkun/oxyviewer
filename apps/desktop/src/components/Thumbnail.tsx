import { useQuery } from "@tanstack/react-query";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  generatedPreview,
  isTauri,
  previewUrl,
  renewMediaResource,
} from "../lib/api";
import {
  browserImageSourceWhenEnabled,
  getBrowserImageSize,
  firstReadyBrowserImage,
  markBrowserImageReady,
  touchBrowserImage,
} from "../lib/browserImageCache";
import { perfMark } from "../lib/perfProbe";
import { retainMediaResource, releaseUnretainedMediaResource } from "../lib/mediaResourceLease";
import { imageProjectionKey, useImageProjectionStore } from "../lib/imageProjection";
import {
  assetRenderQueryKey,
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
  rank?: number;
  onContextMenu?: React.MouseEventHandler<HTMLDivElement>;
  onImageLoad?: (size: { width: number; height: number }) => void;
  onRawPreviewStatus?: (status: RawPreviewStatus) => void;
}

interface DisplayedImage {
  assetId: string;
  source: string;
  resourceId?: string;
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
  rank = 0,
  onContextMenu,
  onImageLoad,
  onRawPreviewStatus,
}: ThumbnailProps) {
  const [failed, setFailed] = useState(false);
  const [fullImageFailed, setFullImageFailed] = useState(false);
  const [loaded, setLoaded] = useState<{ assetId: string; mode: "preview" | "full" }>();
  const [displayedImage, setDisplayedImage] = useState<DisplayedImage>();
  const imageDebug = useRef<{ source: string; handle: PreviewDebugHandle } | undefined>(undefined);
  const reportedLoads = useRef(new Set<string>());
  const plan = useMemo(
    () => renderPlan(asset.kind, large ? "loupe" : "thumbnail"),
    [asset.kind, large],
  );
  const previewStep = plan[0];
  const fullStep = plan.find((step) => step.level === "full");
  const previewMethod = previewStep.method;
  const fullMethod = fullStep?.method;
  const previewLevel = generatedLevel(previewMethod);
  const fullLevel = generatedLevel(fullMethod);
  const previewMethodIdentity = renderMethodKey(previewMethod);
  const fullMethodIdentity = fullMethod ? renderMethodKey(fullMethod) : undefined;
  const distinctFullLevel = fullMethodIdentity !== previewMethodIdentity ? fullLevel : undefined;
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
        rank,
      );
      // The projection store owns image data; null records successful query
      // completion without React Query treating undefined as a failed request.
      return null;
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
        rank,
      );
      // The projection store owns image data; null records successful query
      // completion without React Query treating undefined as a failed request.
      return null;
    },
    enabled: enabled
      && isTauri()
      && large
      && Boolean(distinctFullLevel)
      && Boolean(previewProjection?.result || previewQuery.isError || directSource),
    staleTime: Infinity,
    retry: 0,
  });
  const refetchPreview = previewQuery.refetch;
  const refetchFull = fullQuery.refetch;
  const previewSource = previewProjection?.result;
  const fullSource = fullProjection?.result;
  const preparedSource = firstReadyBrowserImage([
    !fullImageFailed ? fullSource?.url : undefined,
    directSource,
    previewSource?.url,
  ]);
  const resourceForSource = (url: string | undefined) => [previewSource, fullSource]
    .find((result) => result?.url === url)?.resource?.resourceId;
  const preparedResourceId = resourceForSource(preparedSource);
  const preparedSize = preparedSource ? getBrowserImageSize(preparedSource) : undefined;
  const visibleImage = displayedImage?.assetId === asset.id
    ? displayedImage
    : preparedSource
      ? { assetId: asset.id, source: preparedSource, resourceId: preparedResourceId }
      : undefined;
  const generatedSource = nextProgressiveStage(Boolean(visibleImage), [
    previewSource,
    !fullImageFailed ? fullSource : undefined,
  ]);
  const sourceCandidate = directSource ?? generatedSource?.url;
  // A mounted virtual row must not begin filesystem I/O or image decoding
  // while its scroll container is moving. Already-decoded browser images can
  // still paint immediately from the local in-memory cache.
  const source = browserImageSourceWhenEnabled(sourceCandidate, enabled);
  const debugResourceLabel = directSource
    ? "original"
    : generatedSource?.renderLevel ?? previewStep.level;
  const pendingSource = source !== visibleImage?.source ? source : undefined;
  const resourceIds = [visibleImage?.resourceId, !failed && pendingSource ? resourceForSource(pendingSource) : undefined]
    .filter((id): id is string => Boolean(id));
  const resourceIdentity = [...new Set(resourceIds)].sort().join("\u0000");
  const projectionResourceIdentity = [previewSource, fullSource]
    .map((result) => result?.resource?.resourceId).filter(Boolean).join("\u0000");
  const seed = hashSeed(asset.name);
  const style = {
    "--thumb-hue": `${seed}`,
    "--thumb-hue-two": `${(seed + 72) % 360}`,
  } as React.CSSProperties;

  const reportImageLoaded = (
    size: { width: number; height: number },
    result: PreviewResult | undefined,
    loadedSource: string,
  ) => {
    const loadedLevel = result && result === fullSource
      ? fullStep?.level ?? "full"
      : previewStep.level;
    const reportKey = `${asset.id}\u0000${loadedSource}\u0000${loadedLevel}`;
    if (reportedLoads.current.has(reportKey)) return;
    reportedLoads.current.add(reportKey);
    perfMark("image:loaded", {
      assetName: asset.name,
      large,
      stage: directSource ? "direct" : loadedLevel,
      renderLevel: loadedLevel,
      width: size.width,
      height: size.height,
    });
  };

  useLayoutEffect(() => {
    const ids = resourceIdentity ? resourceIdentity.split("\u0000") : [];
    if (ids.length === 0) return;
    const releases = ids.map(retainMediaResource);
    let disposed = false;
    let recovering = false;
    const renew = async () => {
      if (recovering) return;
      const live = await Promise.all(ids.map((id) => renewMediaResource(id).catch(() => false)));
      if (disposed || live.every(Boolean)) return;
      // A bounded registry may evict an unmounted/expired descriptor while its
      // managed file remains valid. Re-enter Rust so it can register a fresh
      // current-process resource instead of retrying the immutable stale URL.
      // Keep stale displayed pixels until the replacement loads, and recover
      // only once per descriptor set instead of retrying its immutable URL.
      recovering = true;
      await Promise.all([
        refetchPreview(),
        ...(large && distinctFullLevel ? [refetchFull()] : []),
      ]);
    };
    void renew();
    const timer = window.setInterval(() => void renew(), 10_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
      releases.forEach((release) => release());
    };
  }, [distinctFullLevel, large, refetchFull, refetchPreview, resourceIdentity]);

  useEffect(() => {
    const retained = new Set(resourceIdentity.split("\u0000"));
    for (const id of projectionResourceIdentity.split("\u0000")) {
      if (id && !retained.has(id)) releaseUnretainedMediaResource(id);
    }
  }, [projectionResourceIdentity, resourceIdentity]);

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
            resourceKey: `image:${source}`,
            resourceLabel: debugResourceLabel,
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
  }, [asset.name, debugResourceLabel, directSource, failed, source]);

  useEffect(() => {
    imageDebug.current?.handle.updatePriority(requestPriority);
  }, [requestPriority]);

  useLayoutEffect(() => {
    if (!preparedSource) return;
    // A cache-ready image paints without the pending image's onLoad handler.
    // Retain that displayed source locally: projection updates and LRU eviction
    // may drop the shared cache entry while this DOM image is still visible.
    // Otherwise React reuses it as a hidden pending image with the same src,
    // which does not reliably fire another load event to reveal it again.
    setDisplayedImage((current) => current?.assetId === asset.id
      ? current
      : { assetId: asset.id, source: preparedSource, resourceId: preparedResourceId });
  }, [asset.id, preparedResourceId, preparedSource]);

  useEffect(() => {
    setFullImageFailed(false);
  }, [asset.id, fullSource?.resource?.resourceId]);

  useLayoutEffect(() => {
    if (!preparedSize || visibleImage?.source !== preparedSource) return;
    if (preparedSource) touchBrowserImage(preparedSource);
    // Virtualized thumbnails must refresh their LRU position when revisited,
    // even while cold resource loading is paused during active scrolling.
    if (!large) return;
    const preparedResult = fullSource?.url === preparedSource
      ? fullSource
      : previewSource?.url === preparedSource
        ? previewSource
        : undefined;
    if (preparedSource) reportImageLoaded(preparedSize, preparedResult, preparedSource);
    onImageLoad?.(preparedSize);
    if (!ownsFullDetailStage || (previewSource?.url !== preparedSource && fullSource?.url !== preparedSource)) return;
    setLoaded((current) => current?.assetId === asset.id
      ? current
      : { assetId: asset.id, mode: fullSource?.url === preparedSource ? "full" : "preview" });
  }, [
    asset.id,
    asset.kind,
    large,
    onImageLoad,
    ownsFullDetailStage,
    visibleImage?.source,
    preparedSize,
    preparedSource,
    fullSource,
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

  const handleLoad = (
    size: { width: number; height: number },
    result: PreviewResult | undefined,
    image: HTMLImageElement,
  ) => {
    if (source) reportImageLoaded(size, result, source);
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
            resourceKey: `image:${source}`,
            resourceLabel: debugResourceLabel,
          })
        : undefined;
      debug?.start();
      if (debug) imageDebug.current = { source, handle: debug };
    }
    debug?.complete();
    if (source) markBrowserImageReady(source, size, image);
    if (ownsFullDetailStage && large && result?.renderLevel !== "thumbnail") {
      setLoaded({ assetId: asset.id, mode: result === fullSource ? "full" : "preview" });
    }
    if (source) setDisplayedImage({ assetId: asset.id, source, resourceId: result?.resource?.resourceId });
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
            resourceKey: `image:${source}`,
            resourceLabel: debugResourceLabel,
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
      aria-busy={!visibleImage && !failed}
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
          }, generatedSource, event.currentTarget)}
        />
      ) : null}
      {asset.kind === "raw" ? <span className="thumbnail__badge">RAW</span> : null}
    </div>
  );
}
