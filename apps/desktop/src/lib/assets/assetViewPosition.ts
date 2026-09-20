import type { AssetSummary } from "@/types";

export function activeAssetIndex(
  assets: readonly Pick<AssetSummary, "id">[],
  activeId: string | undefined,
): number | undefined {
  if (!activeId) return undefined;
  const index = assets.findIndex((asset) => asset.id === activeId);
  return index >= 0 ? index : undefined;
}

/** 1-based position of the active asset, for "n / total" readouts. */
export function activeAssetOrdinal(
  assets: readonly Pick<AssetSummary, "id">[],
  activeId: string | undefined,
): number | undefined {
  const index = activeAssetIndex(assets, activeId);
  return index === undefined ? undefined : index + 1;
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
  return replacementAssetIdAfterRemovals(assets, [removedId], removedId);
}

export function replacementAssetIdAfterRemovals(
  assets: readonly Pick<AssetSummary, "id">[],
  removedIds: readonly string[],
  anchorId: string | undefined,
): string | undefined {
  const removed = new Set(removedIds);
  const anchorIndex = anchorId ? assets.findIndex((asset) => asset.id === anchorId) : -1;
  if (anchorIndex < 0 || !removed.has(assets[anchorIndex].id)) return undefined;
  for (let index = anchorIndex + 1; index < assets.length; index += 1) {
    if (!removed.has(assets[index].id)) return assets[index].id;
  }
  for (let index = anchorIndex - 1; index >= 0; index -= 1) {
    if (!removed.has(assets[index].id)) return assets[index].id;
  }
  return undefined;
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
