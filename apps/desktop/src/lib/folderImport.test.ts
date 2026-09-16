import { describe, expect, it } from "vitest";
import {
  applyEntryFailure,
  applyEntryOpened,
  applyEntryRetried,
  applyIndexCompleted,
  attachImportRoot,
  beginFolderImport,
  dismissFolderImport,
  failFolderImport,
  failedImportEntries,
  folderImportSummary,
  folderName,
  isFolderImportPending,
} from "./folderImport";
import type { FolderImportState } from "./folderImport";

/** The pipeline as App runs it: begin, then bind and open each path in order. */
function openedState(paths: string[], roots = paths.map((path) => `/canonical${path}`)): FolderImportState {
  let state = beginFolderImport(paths);
  paths.forEach((_, index) => {
    state = attachImportRoot(state, index, roots[index]);
    state = applyEntryOpened(state, roots[index]);
  });
  return state;
}

describe("folder import state machine", () => {
  it("starts every dropped folder as registering, with no resolution pass", () => {
    const state = beginFolderImport(["/photos/2024/", "C:\\trips\\coast"]);

    expect(state.visible).toBe(true);
    expect(state.entries.map((entry) => [entry.name, entry.stage, entry.rootPath])).toEqual([
      ["2024", "registering", undefined],
      ["coast", "registering", undefined],
    ]);
    expect(isFolderImportPending(state)).toBe(true);
  });

  it("binds the canonical root the backend returns and moves to indexing", () => {
    const bound = attachImportRoot(beginFolderImport(["/photos/2024"]), 0, "/photos/2024");

    expect(bound.entries[0].rootPath).toBe("/photos/2024");
    expect(bound.entries[0].stage).toBe("registering");
    expect(applyEntryOpened(bound, "/photos/2024").entries[0].stage).toBe("indexing");
    // A row that already carries a root is left alone.
    expect(attachImportRoot(bound, 0, "/other").entries[0].rootPath).toBe("/photos/2024");
  });

  it("counts photos only when the completion event arrives", () => {
    const opened = openedState(["/photos"]);
    expect(opened.entries[0]).toMatchObject({ stage: "indexing", assetCount: 0 });

    const done = applyIndexCompleted(opened, {
      rootPath: "/canonical/photos",
      assetCount: 300,
      directoryCount: 9,
    });
    expect(done.entries[0]).toMatchObject({ stage: "ready", assetCount: 300 });
    // A completion for another root is not this import's business.
    expect(applyIndexCompleted(opened, { rootPath: "/elsewhere", assetCount: 1, directoryCount: 1 })).toBe(opened);
  });

  it("keeps the first completion result when a later one repeats", () => {
    const ready = applyIndexCompleted(openedState(["/photos"]), {
      rootPath: "/canonical/photos",
      assetCount: 1_200,
      directoryCount: 40,
    });

    expect(ready.entries[0]).toMatchObject({ stage: "ready", assetCount: 1_200 });
    expect(applyIndexCompleted(ready, {
      rootPath: "/canonical/photos",
      assetCount: 1_200,
      directoryCount: 40,
    }).entries[0].assetCount).toBe(1_200);
  });

  it("reports a failed folder and can add a row for a path that never opened", () => {
    const opened = openedState(["/photos"]);

    const failed = applyEntryFailure(opened, "/photos", "Permission denied");
    expect(failed.entries[0]).toMatchObject({ stage: "failed", error: "Permission denied" });

    const unknown = applyEntryFailure(dismissFolderImport(), "/nas/archive", "unavailable");
    expect(unknown.visible).toBe(true);
    expect(unknown.entries[0]).toMatchObject({ name: "archive", stage: "failed" });
  });

  it("revives a failed row when its retry succeeds", () => {
    const failed = applyEntryFailure(openedState(["/photos"]), "/canonical/photos", "Permission denied");
    expect(failed.entries[0]).toMatchObject({ stage: "failed", error: "Permission denied" });

    const retried = applyEntryRetried(failed, "/canonical/photos");
    expect(retried.entries[0].stage).toBe("indexing");
    expect(retried.entries[0].error).toBeUndefined();
    // A ready row must not be dragged back into the pipeline by a stray retry.
    const ready = applyIndexCompleted(failed, { rootPath: "/canonical/photos", assetCount: 1, directoryCount: 1 });
    expect(applyEntryRetried(ready, "/canonical/photos").entries[0].stage).toBe("ready");
  });

  it("summarizes imported, failed, and photo counts", () => {
    let state = openedState(["/photos", "/trips"]);
    state = applyIndexCompleted(state, { rootPath: "/canonical/photos", assetCount: 100, directoryCount: 4 });
    state = applyEntryFailure(state, "/trips", "gone");

    expect(folderImportSummary(state)).toEqual({
      total: 2,
      imported: 1,
      failed: 1,
      assetCount: 100,
      pending: false,
    });
    expect(isFolderImportPending(state)).toBe(false);
    expect(failedImportEntries(state).map((entry) => entry.name)).toEqual(["trips"]);
  });

  it("reports a whole-import failure and derived names", () => {
    expect(failFolderImport("boom")).toMatchObject({ visible: true, error: "boom" });
    expect(folderName("/a/b/c/")).toBe("c");
    expect(folderName("C:\\a\\b")).toBe("b");
    expect(folderName("folder")).toBe("folder");
  });
});
