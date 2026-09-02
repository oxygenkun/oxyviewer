import { Star } from "lucide-react";
import type { AssetSummary } from "../types";

export function AssetMetadataBadges({ asset }: { asset: AssetSummary }) {
  if (!asset.rating && !asset.colorLabel) return null;
  return (
    <span className="asset-metadata-badges">
      {asset.rating ? (
        <span className="asset-rating-badge" title={`${asset.rating} / 5`}>
          <Star aria-hidden="true" size={11} />
          <b>{asset.rating}</b>
        </span>
      ) : null}
      {asset.colorLabel ? (
        <b
          className="asset-color-label"
          style={{ background: `var(--label-${asset.colorLabel.toLowerCase()})` }}
          title={asset.colorLabel}
        />
      ) : null}
    </span>
  );
}
