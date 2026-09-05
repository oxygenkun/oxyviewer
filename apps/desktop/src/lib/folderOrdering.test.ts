import { describe, expect, it } from "vitest";
import type { FolderSession } from "../types";
import { mergeVisibleFolderOrder, moveFolderRelative, sortFolderSessions } from "./folderOrdering";

const folder = (displayName: string, rootPath = `/${displayName}`): FolderSession => ({
  id: rootPath,
  rootPath,
  displayName,
  openedAtMs: 0,
  deletionMode: "trash",
});

describe("folder ordering", () => {
  it("keeps the imported order without copying the list", () => {
    const sessions = [folder("Zulu"), folder("Alpha")];
    expect(sortFolderSessions(sessions, "import", "en")).toBe(sessions);
  });

  it("sorts names naturally in either direction", () => {
    const sessions = [folder("Trip 10"), folder("Trip 2"), folder("Archive")];
    expect(sortFolderSessions(sessions, "nameAscending", "en").map((item) => item.displayName))
      .toEqual(["Archive", "Trip 2", "Trip 10"]);
    expect(sortFolderSessions(sessions, "nameDescending", "en").map((item) => item.displayName))
      .toEqual(["Trip 10", "Trip 2", "Archive"]);
  });

  it("moves a dragged folder into the target's upper or lower interval", () => {
    expect(moveFolderRelative(["a", "b", "c", "d"], "d", "b", "before"))
      .toEqual(["a", "d", "b", "c"]);
    expect(moveFolderRelative(["a", "b", "c", "d"], "a", "c", "after"))
      .toEqual(["b", "c", "a", "d"]);
    const unchanged = ["a", "b", "c"];
    expect(moveFolderRelative(unchanged, "b", "c", "before")).toBe(unchanged);
  });

  it("preserves hidden library roots while reordering visible folders", () => {
    expect(mergeVisibleFolderOrder(
      ["visible-a", "offline-a", "visible-b", "offline-b", "visible-c"],
      ["visible-c", "visible-a", "visible-b"],
    )).toEqual(["visible-c", "offline-a", "visible-a", "offline-b", "visible-b"]);
  });
});
