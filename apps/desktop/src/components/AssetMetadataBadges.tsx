import { Star } from "lucide-react";
import type { AssetSummary } from "../types";
import { PickFlagIcon } from "./PickFlagIcon";

export function AssetMetadataBadges({ asset }: { asset: AssetSummary }) {
  if (!asset.rating && !asset.colorLabel && !asset.pickLabel) return null;
  return (
    <span className="asset-metadata-badges">
      {asset.pickLabel ? (
        <span
          className={`asset-pick-label asset-pick-label--${asset.pickLabel}`}
          title={asset.pickLabel}
        >
          <PickFlagIcon value={asset.pickLabel} size={12} />
        </span>
      ) : null}
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
