import {
  failedImportEntries,
  folderImportSummary,
  type FolderImportEntry,
  type FolderImportState,
} from "./folderImport";
import type { MessageKey } from "@/lib/i18n";

/** Content of the status-bar message; the status bar is the only import surface. */
export interface FolderImportNotice {
  kind: "error" | "status";
  message: string;
  detail?: string;
  /** Offers the retry action for every failed row. */
  retry: boolean;
}

type Translate = (key: MessageKey) => string;

function stageLabel(entry: FolderImportEntry, t: Translate): string {
  switch (entry.stage) {
    case "registering":
      return t("importRegistering");
    case "indexing":
      return t("importIndexing");
    case "ready":
      return t("importReady").replace("{count}", entry.assetCount.toLocaleString());
    case "failed":
      return entry.error ?? t("importFailed");
  }
}

/** One line per folder, shown when the user expands the status-bar message. */
export function folderImportDetail(
  state: FolderImportState,
  t: Translate,
): string | undefined {
  const lines = state.entries.map((entry) => `${entry.name} — ${stageLabel(entry, t)}`);
  return lines.length ? lines.join("\n") : undefined;
}

/**
 * Status-bar content for an import: one short line from the moment the folders
 * are accepted until the outcome is read, with the per-folder detail in the
 * expandable panel every notice already has. Index progress is deliberately not
 * parsed; only the completion event contributes counts.
 */
export function folderImportNotice(
  state: FolderImportState,
  t: Translate,
): FolderImportNotice | undefined {
  if (!state.visible) return undefined;
  const summary = folderImportSummary(state);
  const detail = folderImportDetail(state, t);
  if (summary.pending) {
    // No live progress is parsed: the line says what is happening, and the final
    // photo count arrives with the completion event.
    return {
      kind: "status",
      message: t("importTitle").replace("{count}", String(summary.total)),
      detail,
      retry: false,
    };
  }
  const failed = failedImportEntries(state).length;
  return {
    kind: state.error || failed ? "error" : "status",
    message: state.error
      ?? (failed
        ? t("importSummaryFailed")
            .replace("{folders}", String(summary.imported))
            .replace("{failed}", String(failed))
        : t("importSummary")
            .replace("{folders}", String(summary.imported))
            .replace("{assets}", summary.assetCount.toLocaleString())),
    detail,
    retry: failed > 0,
  };
}
