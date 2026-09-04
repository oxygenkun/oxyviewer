import { describe, expect, it } from "vitest";
import type { DirectoryTreeSnapshot, FolderSession } from "../types";
import { acceptDirectoryTreeSnapshot, directoryTreePlaceholder } from "./directoryTreeProjection";

const session: FolderSession = {
  id: "session-1",
  rootPath: "/photos",
  displayName: "photos",
  openedAtMs: 1,
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
});
