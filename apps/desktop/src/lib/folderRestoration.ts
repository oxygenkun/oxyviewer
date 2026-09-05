import type { FolderSession } from "../types";

export interface FolderRestoreState {
  rootPath: string;
  status: "restoring" | "ready" | "failed";
  error?: string;
}

// Publish each result independently: an offline root must not hide ready roots.
export async function restoreFoldersProgressively(
  roots: string[],
  activeRoot: string | undefined,
  open: (path: string) => Promise<FolderSession>,
  publish: (session: FolderSession) => void,
  report: (states: FolderRestoreState[]) => void,
): Promise<void> {
  const states: FolderRestoreState[] = [...new Set(roots)].map((rootPath) => ({
    rootPath, status: "restoring",
  }));
  const notify = () => report(states.map((state) => ({ ...state })));
  notify();
  const prioritized = [...states].sort((a, b) =>
    Number(b.rootPath === activeRoot) - Number(a.rootPath === activeRoot));
  await Promise.all(prioritized.map(async (state) => {
    try {
      const session = await open(state.rootPath);
      publish(session);
      state.status = "ready";
    } catch (cause) {
      state.status = "failed";
      state.error = String(cause);
    }
    notify();
  }));
}

export function insertRestoredFolder(
  current: FolderSession[], session: FolderSession, roots: string[],
): FolderSession[] {
  if (current.some((item) => item.rootPath === session.rootPath)) return current;
  const rank = roots.indexOf(session.rootPath);
  const next = current.findIndex((item) => {
    const index = roots.indexOf(item.rootPath);
    return index < 0 || index > rank;
  });
  const result = [...current];
  result.splice(next < 0 ? result.length : next, 0, session);
  return result;
}
