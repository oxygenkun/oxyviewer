import type { FolderSession } from "../types";

export type FolderSort = "import" | "nameAscending" | "nameDescending";

export function sortFolderSessions(
  sessions: FolderSession[],
  sort: FolderSort,
  locale: string,
): FolderSession[] {
  if (sort === "import") return sessions;
  const multiplier = sort === "nameAscending" ? 1 : -1;
  return [...sessions].sort((left, right) => {
    const byName = left.displayName.localeCompare(right.displayName, locale, {
      numeric: true,
      sensitivity: "base",
    });
    return (byName || left.rootPath.localeCompare(right.rootPath)) * multiplier;
  });
}

export type FolderDropPlacement = "before" | "after";

export function moveFolderRelative(
  paths: string[],
  draggedPath: string,
  targetPath: string,
  placement: FolderDropPlacement,
): string[] {
  const from = paths.indexOf(draggedPath);
  if (from < 0 || !paths.includes(targetPath) || draggedPath === targetPath) return paths;
  const reordered = paths.filter((path) => path !== draggedPath);
  const targetIndex = reordered.indexOf(targetPath);
  reordered.splice(targetIndex + (placement === "after" ? 1 : 0), 0, draggedPath);
  if (reordered.every((path, index) => path === paths[index])) return paths;
  return reordered;
}

export function mergeVisibleFolderOrder(allPaths: string[], visibleOrder: string[]): string[] {
  const visible = new Set(visibleOrder);
  if (visible.size !== visibleOrder.length || visibleOrder.some((path) => !allPaths.includes(path))) {
    return allPaths;
  }
  let visibleIndex = 0;
  return allPaths.map((path) => visible.has(path) ? visibleOrder[visibleIndex++] : path);
}
