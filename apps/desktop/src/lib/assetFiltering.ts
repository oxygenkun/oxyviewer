import type { AssetQuery, AssetSummary } from "../types";

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

function compareAssets(left: AssetSummary, right: AssetSummary, query: AssetQuery): number {
  let ordering = 0;
  if (query.sort === "modified") ordering = left.modifiedAtMs - right.modifiedAtMs;
  else if (query.sort === "size") ordering = left.sizeBytes - right.sizeBytes;
  else if (query.sort === "kind") ordering = compareText(left.kind, right.kind);
  else ordering = compareText(left.name.toLocaleLowerCase(), right.name.toLocaleLowerCase());
  if (ordering === 0) ordering = compareText(left.name, right.name);
  return query.direction === "descending" ? -ordering : ordering;
}

export function filterAndSortAssets(
  assets: readonly AssetSummary[],
  query: AssetQuery,
): AssetSummary[] {
  const search = query.search?.toLocaleLowerCase();
  return assets
    .filter((asset) => !query.kind || asset.kind === query.kind)
    .filter((asset) => !query.minimumRating || (asset.rating ?? 0) >= query.minimumRating)
    .filter((asset) => !query.colorLabels?.length || query.colorLabels.some((label) =>
      asset.colorLabel?.toLocaleLowerCase() === label.toLocaleLowerCase()))
    .filter((asset) => !search || asset.name.toLocaleLowerCase().includes(search))
    .sort((left, right) => compareAssets(left, right, query));
}
