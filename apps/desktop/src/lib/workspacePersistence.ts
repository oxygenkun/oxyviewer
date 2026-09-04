import type { FolderSort } from "./folderOrdering";
import { clampLayoutSize, LAYOUT_SIZE_LIMITS, type LayoutRegion } from "./layoutSizing";

const WORKSPACE_KEY = "oxyviewer.workspace.v1";
const ONBOARDING_KEY = "oxyviewer.folder-onboarding.v1";
const FOCUS_AREAS_KEY = "oxyviewer.focus-areas-visible.v1";
const LOUPE_CONTROLS_AUTO_HIDE_KEY = "oxyviewer.loupe-controls-auto-hide.v1";
const UI_FONT_SCALE_KEY = "oxyviewer.ui-font-scale.v1";
export const UI_FONT_SCALES = [0.8, 1, 1.25, 1.5, 1.75] as const;
export type UiFontScale = (typeof UI_FONT_SCALES)[number];
const LAYOUT_SIZE_KEYS: Record<LayoutRegion, string> = {
  leftPanel: "oxyviewer.left-panel-width.v1",
  inspector: "oxyviewer.inspector-width.v1",
  filmstrip: "oxyviewer.loupe-filmstrip-height.v1",
};
const METADATA_VISIBILITY_KEYS = {
  grid: "oxyviewer.grid-metadata-visible.v1",
  loupe: "oxyviewer.loupe-metadata-visible.v1",
} as const;

export interface WorkspaceSnapshot {
  activeRoot?: string;
  currentDirectories: Record<string, string>;
  folderSort?: FolderSort;
  folderDragEnabled?: boolean;
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
      folderSort: parsed.folderSort === "import" || parsed.folderSort === "nameAscending" ||
        parsed.folderSort === "nameDescending" ? parsed.folderSort : undefined,
      folderDragEnabled: typeof parsed.folderDragEnabled === "boolean"
        ? parsed.folderDragEnabled
        : undefined,
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

export function recoverMissingCurrentDirectory(
  snapshot: WorkspaceSnapshot,
  rootPath: string,
  failedDirectory: string,
): WorkspaceSnapshot {
  if (snapshot.currentDirectories[rootPath] !== failedDirectory) return snapshot;
  return {
    ...snapshot,
    currentDirectories: { ...snapshot.currentDirectories, [rootPath]: rootPath },
  };
}

export function hasSeenFolderOnboarding(storage: StorageLike = window.localStorage): boolean {
  return storage.getItem(ONBOARDING_KEY) === "done";
}

export function completeFolderOnboarding(storage: StorageLike = window.localStorage): void {
  storage.setItem(ONBOARDING_KEY, "done");
}

export function loadFocusAreasVisible(storage?: StorageLike): boolean {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return false;
  try {
    return resolved.getItem(FOCUS_AREAS_KEY) === "true";
  } catch {
    return false;
  }
}

export function saveFocusAreasVisible(visible: boolean, storage?: StorageLike): void {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return;
  try {
    resolved.setItem(FOCUS_AREAS_KEY, String(visible));
  } catch {
    // Preferences must never prevent the viewer from opening (private mode,
    // disabled storage, or a full quota).
  }
}

export function loadLoupeControlsAutoHide(storage?: StorageLike): boolean {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return false;
  try {
    return resolved.getItem(LOUPE_CONTROLS_AUTO_HIDE_KEY) === "true";
  } catch {
    return false;
  }
}

export function saveLoupeControlsAutoHide(enabled: boolean, storage?: StorageLike): void {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return;
  try {
    resolved.setItem(LOUPE_CONTROLS_AUTO_HIDE_KEY, String(enabled));
  } catch {
    // Display preferences must never prevent the viewer from opening.
  }
}

export function loadUiFontScale(storage?: StorageLike): UiFontScale {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return 1;
  try {
    const stored = Number(resolved.getItem(UI_FONT_SCALE_KEY));
    if (stored === 0.85) return 0.8;
    if (stored === 1.15) return 1.25;
    if (stored === 2) return 1.75;
    return UI_FONT_SCALES.find((scale) => scale === stored) ?? 1;
  } catch {
    return 1;
  }
}

export function saveUiFontScale(scale: UiFontScale, storage?: StorageLike): void {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return;
  try {
    resolved.setItem(UI_FONT_SCALE_KEY, String(scale));
  } catch {
    // Display preferences must never prevent the viewer from opening.
  }
}

export function loadLayoutSize(region: LayoutRegion, storage?: StorageLike): number {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return LAYOUT_SIZE_LIMITS[region].defaultValue;
  try {
    const stored = resolved.getItem(LAYOUT_SIZE_KEYS[region]);
    return stored === null
      ? LAYOUT_SIZE_LIMITS[region].defaultValue
      : clampLayoutSize(region, Number(stored));
  } catch {
    return LAYOUT_SIZE_LIMITS[region].defaultValue;
  }
}

export function saveLayoutSize(
  region: LayoutRegion,
  value: number,
  storage?: StorageLike,
): void {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return;
  try {
    resolved.setItem(LAYOUT_SIZE_KEYS[region], String(clampLayoutSize(region, value)));
  } catch {
    // Layout preferences must never prevent the viewer from opening.
  }
}

export function loadMetadataVisibility(
  view: keyof typeof METADATA_VISIBILITY_KEYS,
  storage?: StorageLike,
): boolean {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return true;
  try {
    return resolved.getItem(METADATA_VISIBILITY_KEYS[view]) !== "false";
  } catch {
    return true;
  }
}

export function saveMetadataVisibility(
  view: keyof typeof METADATA_VISIBILITY_KEYS,
  visible: boolean,
  storage?: StorageLike,
): void {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return;
  try {
    resolved.setItem(METADATA_VISIBILITY_KEYS[view], String(visible));
  } catch {
    // Display preferences must never prevent the viewer from opening.
  }
}
