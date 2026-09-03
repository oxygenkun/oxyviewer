import type { AssetSummary } from "../types";

export function activeAssetIndex(
  assets: readonly Pick<AssetSummary, "id">[],
  activeId: string | undefined,
): number | undefined {
  if (!activeId) return undefined;
  const index = assets.findIndex((asset) => asset.id === activeId);
  return index >= 0 ? index : undefined;
}

export function gridRowForAsset(assetIndex: number, columns: number): number {
  return Math.floor(assetIndex / columns);
}
