import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { generatedPreviewUrl, isTauri, previewUrl } from "../lib/api";
import { previewStages } from "../lib/preview";
import type { AssetSummary } from "../types";

interface ThumbnailProps {
  asset: AssetSummary;
  large?: boolean;
  onImageLoad?: (size: { width: number; height: number }) => void;
}

function hashSeed(value: string) {
  let seed = 0;
  for (let index = 0; index < value.length; index += 1) {
    seed = (seed * 31 + value.charCodeAt(index)) % 360;
  }
  return seed;
}

export function Thumbnail({ asset, large = false, onImageLoad }: ThumbnailProps) {
  const [failed, setFailed] = useState(false);
  const directSource = useMemo(() => previewUrl(asset), [asset]);
  const stages = previewStages(large);
  const thumbnailSize = stages[0];
  const loupeSize = stages.length > 1 ? stages[1] : undefined;
  const thumbnailSource = useQuery({
    queryKey: ["asset-preview", asset.id, asset.modifiedAtMs, thumbnailSize],
    queryFn: () => generatedPreviewUrl(asset, thumbnailSize),
    enabled: isTauri() && !directSource,
    staleTime: Infinity,
    retry: 0,
  });
  const loupeSource = useQuery({
    queryKey: ["asset-preview", asset.id, asset.modifiedAtMs, loupeSize],
    queryFn: () => generatedPreviewUrl(asset, loupeSize ?? thumbnailSize),
    enabled: isTauri() && !directSource && Boolean(loupeSize && thumbnailSource.data),
    staleTime: Infinity,
    retry: 0,
  });
  const source = directSource ?? loupeSource.data ?? thumbnailSource.data;
  const seed = hashSeed(asset.name);
  const style = {
    "--thumb-hue": `${seed}`,
    "--thumb-hue-two": `${(seed + 72) % 360}`,
  } as React.CSSProperties;

  useEffect(() => setFailed(false), [source]);

  return (
    <div className={`thumbnail ${large ? "thumbnail--large" : ""}`} style={style}>
      {source && !failed ? (
        <img
          src={source}
          alt=""
          draggable={false}
          onError={() => setFailed(true)}
          onLoad={(event) => onImageLoad?.({
            width: event.currentTarget.naturalWidth,
            height: event.currentTarget.naturalHeight,
          })}
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
