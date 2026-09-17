import type { LibraryIndexUpdate } from "@/types";

/**
 * One imported folder moving through the pipeline. `droppedPath` is what the OS
 * or the folder picker handed over; `rootPath` is the canonical root once the
 * backend has opened it.
 */
export interface FolderImportEntry {
  droppedPath: string;
  rootPath?: string;
  name: string;
  stage: FolderImportStage;
  /** Final photo count, reported once by the index completion event. */
  assetCount: number;
  error?: string;
}

export type FolderImportStage =
  | "registering"
  | "indexing"
  | "ready"
  | "failed";

export interface FolderImportState {
  entries: FolderImportEntry[];
  /** A process is showing; it stays until it settles or is dismissed. */
  visible: boolean;
  /** Whole-import failure, e.g. the folder picker itself failed. */
  error?: string;
}

export const IDLE_FOLDER_IMPORT: FolderImportState = {
  entries: [],
  visible: false,
};

export interface FolderImportSummary {
  total: number;
  /** Folders opened by this import and confirmed ready. */
  imported: number;
  failed: number;
  assetCount: number;
  pending: boolean;
}

const WORKING_STAGES: FolderImportStage[] = ["registering", "indexing"];

export function folderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}

/**
 * Start importing the given folders. This is the same work the folder picker has
 * always done — open, register, browse — with progress narration added, so there
 * is no extra resolution or relationship analysis between the paths.
 */
export function beginFolderImport(paths: string[]): FolderImportState {
  return {
    entries: paths.map((droppedPath) => ({
      droppedPath,
      name: folderName(droppedPath),
      stage: "registering",
      assetCount: 0,
    })),
    visible: true,
  };
}

export function failFolderImport(error: string): FolderImportState {
  return { entries: [], visible: true, error };
}

/**
 * Bind the canonical root the backend returned to the row for the path at
 * `index`. Rows are created in payload order, so the index is the whole mapping.
 */
export function attachImportRoot(
  state: FolderImportState,
  index: number,
  rootPath: string,
): FolderImportState {
  return {
    ...state,
    entries: state.entries.map((entry, entryIndex) =>
      entryIndex === index && entry.rootPath === undefined ? { ...entry, rootPath } : entry),
  };
}

/** The root is open and indexing has been scheduled. */
export function applyEntryOpened(
  state: FolderImportState,
  rootPath: string,
): FolderImportState {
  return {
    ...state,
    entries: state.entries.map((entry) =>
      entry.rootPath === rootPath && entry.stage === "registering"
        ? { ...entry, stage: "indexing" }
        : entry),
  };
}

/**
 * Put a row back into the pipeline after its retry succeeded. Unlike
 * [`applyEntryOpened`] this also revives a `failed` row and clears its error.
 */
export function applyEntryRetried(
  state: FolderImportState,
  rootPath: string,
): FolderImportState {
  return {
    ...state,
    entries: state.entries.map((entry) =>
      entry.rootPath === rootPath && (entry.stage === "failed" || entry.stage === "registering")
        ? { ...entry, stage: "indexing", error: undefined }
        : entry),
  };
}

export function applyEntryFailure(
  state: FolderImportState,
  path: string,
  error: string,
): FolderImportState {
  const matches = (entry: FolderImportEntry) => entry.rootPath === path || entry.droppedPath === path;
  const known = state.entries.some(matches);
  const entries = known
    ? state.entries.map((entry) => (matches(entry) ? { ...entry, stage: "failed" as const, error } : entry))
    : [
        ...state.entries,
        {
          droppedPath: path,
          rootPath: path,
          name: folderName(path),
          stage: "failed" as const,
          assetCount: 0,
          error,
        },
      ];
  return { ...state, entries, visible: true };
}

export function applyIndexCompleted(
  state: FolderImportState,
  update: LibraryIndexUpdate,
): FolderImportState {
  let matched = false;
  const entries = state.entries.map((entry) => {
    if (entry.rootPath !== update.rootPath) return entry;
    matched = true;
    return {
      ...entry,
      stage: "ready" as const,
      assetCount: Math.max(entry.assetCount, update.assetCount),
      error: undefined,
    };
  });
  return matched ? { ...state, entries } : state;
}

export function dismissFolderImport(): FolderImportState {
  return IDLE_FOLDER_IMPORT;
}

export function isFolderImportPending(state: FolderImportState): boolean {
  return state.entries.some((entry) => WORKING_STAGES.includes(entry.stage));
}

export function folderImportSummary(state: FolderImportState): FolderImportSummary {
  return {
    total: state.entries.length,
    imported: state.entries.filter((entry) => entry.stage === "ready").length,
    failed: state.entries.filter((entry) => entry.stage === "failed").length,
    assetCount: state.entries.reduce((total, entry) => total + entry.assetCount, 0),
    pending: isFolderImportPending(state),
  };
}

/** Failed rows, for the status-bar retry action. */
export function failedImportEntries(state: FolderImportState): FolderImportEntry[] {
  return state.entries.filter((entry) => entry.stage === "failed");
}
