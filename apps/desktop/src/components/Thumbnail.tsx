import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { generatedPreview, isTauri, previewUrl } from "../lib/api";
import { previewStages } from "../lib/preview";
import { rawPreviewStatus, type RawPreviewStatus } from "../lib/rawPreview";
import type { AssetSummary, PreviewResult } from "../types";

interface ThumbnailProps {
  asset: AssetSummary;
  enabled?: boolean;
  large?: boolean;
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

export function Thumbnail({
  asset,
  enabled = true,
  large = false,
  onImageLoad,
  onRawPreviewStatus,
}: ThumbnailProps) {
  const [failed, setFailed] = useState(false);
  const [fullImageFailed, setFullImageFailed] = useState(false);
  const [loaded, setLoaded] = useState<{ assetId: string; mode: "preview" | "full" }>();
  const directSource = useMemo(() => previewUrl(asset), [asset]);
  const stages = previewStages(large);
  const thumbnailSize = stages[0];
  const loupeSize = stages.length > 1 ? stages[1] : undefined;
  const thumbnailSource = useQuery({
    queryKey: ["asset-preview", asset.id, asset.modifiedAtMs, thumbnailSize],
    queryFn: () => generatedPreview(asset, "thumbnail", thumbnailSize),
    enabled: enabled && isTauri() && !directSource,
    staleTime: Infinity,
    retry: 0,
  });
  const loupeSource = useQuery({
    queryKey: ["asset-preview", asset.id, asset.modifiedAtMs, loupeSize],
    queryFn: () => generatedPreview(asset, "loupePreview", loupeSize ?? thumbnailSize),
    enabled: enabled && isTauri() && !directSource && Boolean(loupeSize && thumbnailSource.data),
    staleTime: Infinity,
    retry: 0,
  });
  const fullSource = useQuery({
    queryKey: ["asset-preview", asset.id, asset.modifiedAtMs, "fullRaw"],
    queryFn: () => generatedPreview(asset, "fullRaw"),
    enabled: enabled
      && isTauri()
      && asset.kind === "raw"
      && large
      && Boolean(loupeSource.data || loupeSource.isError),
    staleTime: Infinity,
    retry: 0,
  });
  const generatedSource = (!fullImageFailed ? fullSource.data : undefined)
    ?? loupeSource.data
    ?? thumbnailSource.data;
  const source = directSource ?? generatedSource?.url;
  const seed = hashSeed(asset.name);
  const style = {
    "--thumb-hue": `${seed}`,
    "--thumb-hue-two": `${(seed + 72) % 360}`,
  } as React.CSSProperties;

  useEffect(() => setFailed(false), [source]);
  useEffect(() => {
    setLoaded(undefined);
    setFullImageFailed(false);
  }, [asset.id]);

  useEffect(() => {
    if (!onRawPreviewStatus || !large || asset.kind !== "raw") return;
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
    if (asset.kind === "raw" && large && result) {
      setLoaded({ assetId: asset.id, mode: result === fullSource.data ? "full" : "preview" });
    }
    onImageLoad?.(size);
  };

  const handleError = (result?: PreviewResult) => {
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
