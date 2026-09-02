const WORKSPACE_KEY = "oxyviewer.workspace.v1";
const ONBOARDING_KEY = "oxyviewer.folder-onboarding.v1";

export interface WorkspaceSnapshot {
  activeRoot?: string;
  currentDirectories: Record<string, string>;
}

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const emptySnapshot = (): WorkspaceSnapshot => ({ currentDirectories: {} });

export function parseWorkspaceSnapshot(value: string | null): WorkspaceSnapshot {
  if (!value) return emptySnapshot();
  try {
    const parsed = JSON.parse(value) as Partial<WorkspaceSnapshot>;
    const currentDirectories = parsed.currentDirectories;
    if (!currentDirectories || typeof currentDirectories !== "object" || Array.isArray(currentDirectories)) {
      return emptySnapshot();
    }
    return {
      activeRoot: typeof parsed.activeRoot === "string" ? parsed.activeRoot : undefined,
      currentDirectories: Object.fromEntries(
        Object.entries(currentDirectories).filter(
          (entry): entry is [string, string] => typeof entry[1] === "string",
        ),
      ),
    };
  } catch {
    return emptySnapshot();
  }
}

export function loadWorkspace(storage: StorageLike = window.localStorage): WorkspaceSnapshot {
  return parseWorkspaceSnapshot(storage.getItem(WORKSPACE_KEY));
}

export function saveWorkspace(
  snapshot: WorkspaceSnapshot,
  storage: StorageLike = window.localStorage,
): void {
  storage.setItem(WORKSPACE_KEY, JSON.stringify(snapshot));
}

export function hasSeenFolderOnboarding(storage: StorageLike = window.localStorage): boolean {
  return storage.getItem(ONBOARDING_KEY) === "done";
}

export function completeFolderOnboarding(storage: StorageLike = window.localStorage): void {
  storage.setItem(ONBOARDING_KEY, "done");
}
