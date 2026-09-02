import type { InfiniteData } from "@tanstack/react-query";
import type { AssetDetails, AssetSummary, MetadataPatch, Page } from "../types";

function patchSummary(asset: AssetSummary, patch: MetadataPatch): AssetSummary {
  return {
    ...asset,
    ...(patch.rating !== undefined ? { rating: patch.rating ?? undefined } : {}),
    ...(patch.colorLabel !== undefined ? { colorLabel: patch.colorLabel ?? undefined } : {}),
  };
}

export function patchAssetSummaries(
  assets: AssetSummary[] | undefined,
  paths: ReadonlySet<string>,
  patch: MetadataPatch,
): AssetSummary[] | undefined {
  return assets?.map((asset) => paths.has(asset.path) ? patchSummary(asset, patch) : asset);
}

export function patchAssetPages(
  data: InfiniteData<Page<AssetSummary>> | undefined,
  paths: ReadonlySet<string>,
  patch: MetadataPatch,
): InfiniteData<Page<AssetSummary>> | undefined {
  if (!data) return data;
  return {
    ...data,
    pages: data.pages.map((page) => ({
      ...page,
      items: patchAssetSummaries(page.items, paths, patch) ?? page.items,
    })),
  };
}

export function patchAssetDetails(
  details: AssetDetails | undefined,
  paths: ReadonlySet<string>,
  patch: MetadataPatch,
): AssetDetails | undefined {
  if (!details || !paths.has(details.asset.path)) return details;
  return {
    ...details,
    asset: patchSummary(details.asset, patch),
    metadata: {
      ...details.metadata,
      ...(patch.rating !== undefined ? { rating: patch.rating ?? undefined } : {}),
      ...(patch.colorLabel !== undefined ? { colorLabel: patch.colorLabel ?? undefined } : {}),
    },
  };
}
