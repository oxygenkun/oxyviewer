import { describe, expect, it } from "vitest";
import type { DirectoryTreeNode, DirectoryTreeSnapshot, FolderSession } from "../types";
import { acceptDirectoryTreeSnapshot, directoryRevealStep, directoryTreePlaceholder } from "./directoryTreeProjection";

const session: FolderSession = {
  id: "session-1",
  rootPath: "/photos",
  displayName: "photos",
  openedAtMs: 1,
  deletionMode: "trash",
};

describe("directory tree projection", () => {
  it("creates an unloaded root while Rust prepares the first snapshot", () => {
    expect(directoryTreePlaceholder(session)).toEqual({
      sessionId: "session-1",
      revision: 0,
      root: {
        entry: { path: "/photos", name: "photos", hasChildren: true },
        expanded: false,
        children: null,
      },
    });
  });

  it("rejects an older command response", () => {
    const snapshot = (revision: number): DirectoryTreeSnapshot => ({
      ...directoryTreePlaceholder(session),
      revision,
    });
    expect(acceptDirectoryTreeSnapshot(snapshot(3), snapshot(2)).revision).toBe(3);
    expect(acceptDirectoryTreeSnapshot(snapshot(3), snapshot(4)).revision).toBe(4);
  });

  it("reveals a search selection one loaded level at a time, including its own subtree", () => {
    const root = directoryTreePlaceholder(session).root;
    const selected: DirectoryTreeNode = {
      entry: { path: "/photos/travel/selected", name: "selected", hasChildren: true },
      expanded: false,
      children: null,
    };
    const branch: DirectoryTreeNode = {
      entry: { path: "/photos/travel", name: "travel", hasChildren: true },
      expanded: false,
      children: null,
    };
    const path = selected.entry.path;
    expect(directoryRevealStep(root, path)).toEqual({ expand: "/photos" });
    root.expanded = true;
    expect(directoryRevealStep(root, path)).toBe("waiting");
    root.children = [branch];
    expect(directoryRevealStep(root, path)).toEqual({ expand: branch.entry.path });
    branch.expanded = true;
    expect(directoryRevealStep(root, path)).toBe("waiting");
    branch.children = [selected];
    expect(directoryRevealStep(root, path)).toEqual({ expand: path });
    selected.expanded = true;
    expect(directoryRevealStep(root, path)).toBe("done");
  });

  it("finishes for leaves, missing selections, and unrelated roots", () => {
    const root = directoryTreePlaceholder(session).root;
    expect(directoryRevealStep(root, "/photos-other/travel")).toBe("done");
    root.entry.hasChildren = false;
    root.children = [];
    expect(directoryRevealStep(root, "/photos")).toBe("done");
    expect(directoryRevealStep(root, "/photos/deleted")).toBe("done");
  });

  it("follows Windows paths without expanding a sibling with a shared prefix", () => {
    const root: DirectoryTreeNode = {
      entry: { path: "C:\\photos", name: "photos", hasChildren: true },
      expanded: true,
      children: ["trip-old", "trip"].map((name) => ({
        entry: { path: `C:\\photos\\${name}`, name, hasChildren: true },
        expanded: false,
        children: null,
      })),
    };
    expect(directoryRevealStep(root, "C:\\photos\\trip\\selected"))
      .toEqual({ expand: "C:\\photos\\trip" });
  });
});
