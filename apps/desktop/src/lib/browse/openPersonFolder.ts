import type { FolderSession } from "@/types";
import { isSameOrDescendantPath } from "./folderPaths";

/** Reuse the closest open root; verify availability before changing the workspace. */
export async function resolvePersonFolder(
  path: string,
  sessions: FolderSession[],
  check: (path: string) => Promise<void>,
  register: (path: string) => Promise<FolderSession>,
): Promise<{ session: FolderSession; path: string }> {
  await check(path);
  const existing = sessions.filter(s => isSameOrDescendantPath(s.rootPath, path))
    .sort((a, b) => b.rootPath.length - a.rootPath.length)[0];
  if (existing) return { session: existing, path };
  const session = await register(path);
  return { session, path: session.rootPath };
}
