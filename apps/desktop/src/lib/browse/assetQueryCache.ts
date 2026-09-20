import type { InfiniteData } from "@tanstack/react-query";
import type { AssetSummary, Page } from "@/types";

/**
 * Removes a successfully deleted asset from a paged listing immediately.
 * The authoritative refetch can then run without leaving a stale tile visible.
 */
export function removeAssetFromInfiniteData(
  data: InfiniteData<Page<AssetSummary>, unknown> | undefined,
  path: string,
): InfiniteData<Page<AssetSummary>, unknown> | undefined {
  if (!data) return data;

  let removed = false;
  const pages = data.pages.map((page) => {
    const items = page.items.filter((asset) => asset.path !== path);
    if (items.length === page.items.length) return page;
    removed = true;
    return { ...page, items };
  });
  if (!removed) return data;

  return {
    ...data,
    pages: pages.map((page) => ({ ...page, total: Math.max(0, page.total - 1) })),
  };
}
