import type { DirectoryTreeNode, DirectoryTreeSnapshot, FolderSession } from "../types";
import { isSameOrDescendantPath } from "./folderPaths";

// Expand only the selected branch, waiting for each asynchronously loaded level.
export function directoryRevealStep(
  node: DirectoryTreeNode,
  path: string,
): { expand: string } | "waiting" | "done" {
  if (!isSameOrDescendantPath(node.entry.path, path)) return "done";
  if (!node.expanded && node.entry.hasChildren) return { expand: node.entry.path };
  if (node.entry.path === path) return "done";
  if (node.children === null) return node.expanded ? "waiting" : "done";
  const child = node.children.find((entry) => isSameOrDescendantPath(entry.entry.path, path));
  return child ? directoryRevealStep(child, path) : "done";
}

export function acceptDirectoryTreeSnapshot(
  current: DirectoryTreeSnapshot | undefined,
  incoming: DirectoryTreeSnapshot,
) {
  if (current && current.revision >= incoming.revision) return current;
  return incoming;
}

export function directoryTreePlaceholder(session: FolderSession): DirectoryTreeSnapshot {
  return {
    sessionId: session.id,
    revision: 0,
    root: {
      entry: {
        path: session.rootPath,
        name: session.displayName,
        hasChildren: true,
      },
      expanded: false,
      children: null,
    },
  };
}
