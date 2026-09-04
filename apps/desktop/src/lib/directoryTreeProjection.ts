import type { DirectoryTreeSnapshot, FolderSession } from "../types";

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
