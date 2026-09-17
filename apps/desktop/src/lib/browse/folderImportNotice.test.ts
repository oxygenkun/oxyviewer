import { describe, expect, it } from "vitest";
import {
  applyEntryFailure,
  applyEntryOpened,
  applyIndexCompleted,
  attachImportRoot,
  beginFolderImport,
  failFolderImport,
  IDLE_FOLDER_IMPORT,
  type FolderImportState,
} from "./folderImport";
import { folderImportNotice } from "./folderImportNotice";
import { translate } from "@/lib/i18n";

const t = (key: Parameters<typeof translate>[1]) => translate("en", key);

/** The pipeline as App runs it, up to "opened and waiting for the indexer". */
function openedState(paths: string[]): FolderImportState {
  let state = beginFolderImport(paths);
  paths.forEach((path, index) => {
    state = attachImportRoot(state, index, path);
    state = applyEntryOpened(state, path);
  });
  return state;
}

describe("status-bar import notice", () => {
  it("says nothing when there is no import to report", () => {
    expect(folderImportNotice(IDLE_FOLDER_IMPORT, t)).toBeUndefined();
  });

  it("reports the running import without parsing index progress", () => {
    const running = openedState(["/photos", "/trips"]);

    const notice = folderImportNotice(running, t)!;
    expect(notice.kind).toBe("status");
    expect(notice.message).toBe(t("importTitle").replace("{count}", "2"));
    expect(notice.retry).toBe(false);
    // The expanded panel carries the per-folder breakdown instead of a floating card.
    expect(notice.detail!.split("\n")).toHaveLength(2);
    expect(notice.detail).toContain(t("importIndexing"));
  });

  it("stays indeterminate while the folders are still being opened", () => {
    const notice = folderImportNotice(beginFolderImport(["/photos"]), t)!;
    expect(notice.message).toBe(t("importTitle").replace("{count}", "1"));
    expect(notice.detail).toContain(t("importRegistering"));
  });

  it("reports a clean outcome", () => {
    const state = applyIndexCompleted(openedState(["/photos"]), {
      rootPath: "/photos",
      assetCount: 42,
      directoryCount: 4,
    });

    const notice = folderImportNotice(state, t)!;
    expect(notice.kind).toBe("status");
    expect(notice.message)
      .toBe(t("importSummary").replace("{folders}", "1").replace("{assets}", "42"));
    expect(notice.detail).toContain(t("importReady").replace("{count}", "42"));
  });

  it("keeps a mid-import failure in the detail without claiming the import ended", () => {
    const running = applyEntryFailure(openedState(["/photos", "/trips"]), "/trips", "Permission denied");

    const notice = folderImportNotice(running, t)!;
    expect(notice.message).toBe(t("importTitle").replace("{count}", "2"));
    expect(notice.detail).toContain("Permission denied");
    expect(notice.retry).toBe(false);
  });

  it("reports a settled failure and offers a retry", () => {
    const failed = applyEntryFailure(
      applyIndexCompleted(openedState(["/photos", "/trips"]), {
        rootPath: "/photos",
        assetCount: 7,
        directoryCount: 2,
      }),
      "/trips",
      "Permission denied",
    );

    const notice = folderImportNotice(failed, t)!;
    expect(notice.kind).toBe("error");
    expect(notice.message)
      .toBe(t("importSummaryFailed").replace("{folders}", "1").replace("{failed}", "1"));
    expect(notice.retry).toBe(true);
    expect(notice.detail).toContain("Permission denied");
  });

  it("surfaces a whole-import failure", () => {
    expect(folderImportNotice(failFolderImport("boom"), t)).toMatchObject({
      kind: "error",
      message: "boom",
      retry: false,
    });
  });
});
