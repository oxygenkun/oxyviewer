import type { ReactNode } from "react";
import type { OverlayDescriptor, OverlayRegion } from "@/types";

/** Host-owned geometry and markup; descriptors cannot provide executable UI. */
export function RegionOverlay({ descriptor, className, regionClassName, renderRegion, testIdPrefix, hidden = false }: {
  descriptor: OverlayDescriptor;
  className: string;
  regionClassName: (region: OverlayRegion) => string;
  renderRegion?: (region: OverlayRegion) => ReactNode;
  testIdPrefix?: string;
  hidden?: boolean;
}) {
  if (descriptor.coordinateSpace !== "displayNormalized") return null;
  return <div className={className} aria-hidden={hidden || undefined}>
    {descriptor.items.filter(({ rect }) => [rect.x, rect.y, rect.width, rect.height].every(Number.isFinite) && rect.width > 0 && rect.height > 0).map((region) => <div
      key={region.id}
      className={regionClassName(region)}
      data-testid={testIdPrefix ? `${testIdPrefix}${region.id}` : undefined}
      style={{ left: `${region.rect.x * 100}%`, top: `${region.rect.y * 100}%`, width: `${region.rect.width * 100}%`, height: `${region.rect.height * 100}%` }}
    >{renderRegion?.(region)}</div>)}
  </div>;
}
