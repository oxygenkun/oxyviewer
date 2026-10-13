import type { QueryClient } from "@tanstack/react-query";
import { getAssetTagAssignmentsByPath, setAssetCustomTag } from "@/lib/api";

// Serialize manual edits and shortcut toggles, including across view changes.
export const TAG_MUTATION_SCOPE = { id: "asset-tags" };

export async function refreshTagQueries(queryClient: QueryClient): Promise<void> {
  await Promise.all([
    "custom-tags", "asset-tag-assignments", "asset-tag-source-kinds", "tag-sync-status",
    "asset-details", "assets", "preload-assets", "progressive-metadata-assets",
  ].map((key) => queryClient.invalidateQueries({ queryKey: [key] })));
}

export async function toggleAssetCustomTag(paths: string[], tagId: number): Promise<void> {
  // Read the committed assignments rather than a potentially stale UI cache.
  const assignments = await getAssetTagAssignmentsByPath(paths);
  const assigned = new Set(assignments
    .filter((row) => row.assignments.some((item) => item.tag.id === tagId && item.assignedCount > 0))
    .map((row) => row.path));
  const remove = paths.filter((path) => assigned.has(path));
  const add = paths.filter((path) => !assigned.has(path));
  if (remove.length) await setAssetCustomTag(remove, tagId, false);
  if (add.length) await setAssetCustomTag(add, tagId, true);
}
