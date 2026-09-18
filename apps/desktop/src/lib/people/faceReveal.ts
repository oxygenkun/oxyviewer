import { isSameOrDescendantPath, parentFolderPath } from "@/lib/browse/folderPaths";
import type { FaceAssetReveal, FolderSession } from "@/types";

/**
 * What the main window must do to show one face's photo.
 *
 * The workbench runs in a separate window and knows only the observation it
 * clicked, so the main window has to turn that into browse state: find the
 * session that owns the path, switch to its folder, wait for the directory's
 * pages, then select the asset and open the loupe. Keeping that decision pure
 * makes the ordering testable instead of buried in an effect.
 */
export type FaceRevealStep =
  | { kind: "navigate"; session: FolderSession; directory: string }
  | { kind: "select"; assetId: string }
  | { kind: "wait" }
  | { kind: "unresolved"; reason: "outsideLibrary" | "filtered" | "notFound" };

export interface FaceRevealInput {
  reveal: FaceAssetReveal;
  sessions: readonly FolderSession[];
  activeRoot?: string;
  currentDirectory?: string;
  /** Ids the browser currently holds; the target may still be on a later page. */
  visibleAssetIds: readonly string[];
  /** True once the directory has no more pages or pending metadata work. */
  directorySettled: boolean;
  /** Search, tags, rating, color, or flag filters are narrowing the grid. */
  filtersActive: boolean;
}

export function faceRevealStep({
  reveal,
  sessions,
  activeRoot,
  currentDirectory,
  visibleAssetIds,
  directorySettled,
  filtersActive,
}: FaceRevealInput): FaceRevealStep {
  const session = sessions.find((item) => isSameOrDescendantPath(item.rootPath, reveal.assetPath));
  const directory = parentFolderPath(reveal.assetPath);
  // The asset must sit directly in a directory the browser is allowed to list:
  // a session root is the only path it can show, and the grid is non-recursive.
  if (!session || !isSameOrDescendantPath(session.rootPath, directory)) {
    return { kind: "unresolved", reason: "outsideLibrary" };
  }
  if (activeRoot !== session.rootPath || currentDirectory !== directory) {
    return { kind: "navigate", session, directory };
  }
  if (visibleAssetIds.includes(reveal.assetId)) {
    return { kind: "select", assetId: reveal.assetId };
  }
  // Background pagination keeps pulling pages, so waiting is the normal answer
  // until the directory is exhausted.
  if (!directorySettled) return { kind: "wait" };
  return { kind: "unresolved", reason: filtersActive ? "filtered" : "notFound" };
}
