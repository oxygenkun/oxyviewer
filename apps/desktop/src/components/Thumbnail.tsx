import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  generatedPreview,
  isTauri,
  previewUrl,
  raiseGeneratedPreviewPriority,
} from "../lib/api";
import {
  getBrowserImageSize,
  isBrowserImageReady,
  markBrowserImageReady,
} from "../lib/browserImageCache";
import { perfMark } from "../lib/perfProbe";
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
  const previewLevel = generatedLevel(previewMethod);
  const fullLevel = generatedLevel(fullMethod);
  const previewMethodIdentity = renderMethodKey(previewMethod);
  const fullMethodIdentity = fullMethod ? renderMethodKey(fullMethod) : undefined;
  const distinctFullLevel = fullMethodIdentity !== previewMethodIdentity ? fullLevel : undefined;
  const ownsFullDetailStage = asset.kind === "raw"
    || (asset.kind === "heif" && fullMethod?.type === "generatedImage");
  const directSource = useMemo(
    () => previewMethod.type === "originalImage" ? previewUrl(asset) : undefined,
    [asset, previewMethod.type],
  );
  const requestPriority = large ? "loupe" : priority;
  const previewQuery = useQuery({
    queryKey: assetRenderQueryKey(asset, previewMethod),
    queryFn: ({ signal }) => generatedPreview(
      asset,
      previewLevel ?? previewStep.level,
      signal,
      requestPriority,
      queueOrder,
    ),
    enabled: enabled && isTauri() && Boolean(previewLevel),
    staleTime: Infinity,
    retry: 0,
  });
  const fullQuery = useQuery({
    queryKey: fullMethod
      ? assetRenderQueryKey(asset, fullMethod)
      : ["asset-render", asset.id, asset.modifiedAtMs, "no-full-image"],
    queryFn: ({ signal }) => generatedPreview(
      asset,
      distinctFullLevel ?? "full",
      signal,
      "loupe",
      queueOrder,
    ),
    enabled: enabled
      && isTauri()
      && large
      && Boolean(distinctFullLevel)
      && Boolean(previewQuery.data || previewQuery.isError || directSource),
    staleTime: Infinity,
    retry: 0,
  });
  const previewSource = previewQuery.data;
  const preparedSource = directSource && isBrowserImageReady(directSource)
    ? directSource
    : previewSource && isBrowserImageReady(previewSource.url)
      ? previewSource.url
      : undefined;
  const preparedSize = preparedSource ? getBrowserImageSize(preparedSource) : undefined;
  const visibleImage = displayedImage?.assetId === asset.id
    ? displayedImage
    : preparedSource
      ? { assetId: asset.id, source: preparedSource }
      : undefined;
  const generatedSource = nextProgressiveStage(Boolean(visibleImage), [
    previewSource,
    !fullImageFailed ? fullQuery.data : undefined,
  ]);
  const source = directSource ?? generatedSource?.url;
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
    if (previewLevel) {
      raiseGeneratedPreviewPriority(asset, previewLevel, requestPriority, queueOrder);
    }
  }, [asset, previewLevel, queueOrder, requestPriority]);

  useEffect(() => {
    setLoaded(undefined);
    setDisplayedImage(undefined);
    setFullImageFailed(false);
  }, [asset.id]);

  useEffect(() => {
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
      && fullQuery.data?.path
      && fullQuery.data.path === previewSource?.path
    ) {
      setLoaded({ assetId: asset.id, mode: "full" });
    }
  }, [asset.id, fullQuery.data?.path, loaded, previewSource?.path]);

  useEffect(() => {
    // Windows/Linux HEIF status is owned by the tile canvas. macOS HEIF and
    // RAW own a file-backed full stage here.
    if (!onRawPreviewStatus || !large || !ownsFullDetailStage) return;
    onRawPreviewStatus(rawPreviewStatus({
      assetId: asset.id,
      loaded,
      fullError: fullQuery.isError || fullImageFailed,
      fullSize: fullQuery.data,
    }));
  }, [
    asset.id,
    asset.kind,
    fullQuery.data?.height,
    fullQuery.data?.width,
    fullQuery.isError,
    fullImageFailed,
    large,
    loaded,
    onRawPreviewStatus,
    ownsFullDetailStage,
  ]);

  const handleLoad = (size: { width: number; height: number }, result?: PreviewResult) => {
    const loadedLevel = result && result === fullQuery.data
      ? fullStep?.level ?? "full"
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
    if (ownsFullDetailStage && large && result) {
      setLoaded({ assetId: asset.id, mode: result === fullQuery.data ? "full" : "preview" });
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
    if (result && result === fullQuery.data) {
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
