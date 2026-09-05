import type { AssetSummary } from "../types";

export function activeAssetIndex(
  assets: readonly Pick<AssetSummary, "id">[],
  activeId: string | undefined,
): number | undefined {
  if (!activeId) return undefined;
  const index = assets.findIndex((asset) => asset.id === activeId);
  return index >= 0 ? index : undefined;
}

export type FocusRestoreAction = "none" | "fetch" | "wait" | "fallback";

export function focusRestoreAction(
  assets: readonly Pick<AssetSummary, "id">[],
  restoreId: string | undefined,
  hasNextPage: boolean,
  isFetchingNextPage: boolean,
): FocusRestoreAction {
  if (!restoreId || assets.some((asset) => asset.id === restoreId)) return "none";
  if (isFetchingNextPage) return "wait";
  return hasNextPage ? "fetch" : "fallback";
}

export function replacementAssetIdAfterRemoval(
  assets: readonly Pick<AssetSummary, "id">[],
  removedId: string,
): string | undefined {
  const removedIndex = assets.findIndex((asset) => asset.id === removedId);
  if (removedIndex < 0) return undefined;
  return assets[removedIndex + 1]?.id ?? assets[removedIndex - 1]?.id;
}

export function gridRowForAsset(assetIndex: number, columns: number): number {
  return Math.floor(assetIndex / columns);
}

export function virtualAssetCount(loadedCount: number, total: number): number {
  return Math.max(loadedCount, total);
}

export function gridRowCount(assetCount: number, columns: number): number {
  return Math.ceil(assetCount / columns);
}
