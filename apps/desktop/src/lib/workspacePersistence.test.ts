import { describe, expect, it } from "vitest";
import {
  loadFocusAreasVisible,
  parseWorkspaceSnapshot,
  saveFocusAreasVisible,
} from "./workspacePersistence";

describe("workspace persistence", () => {
  it("restores the active root and each root's last directory", () => {
    expect(parseWorkspaceSnapshot(JSON.stringify({
      activeRoot: "/photos",
      currentDirectories: { "/photos": "/photos/2025", "/archive": "/archive" },
    }))).toEqual({
      activeRoot: "/photos",
      currentDirectories: { "/photos": "/photos/2025", "/archive": "/archive" },
    });
  });

  it("fails closed for malformed or obsolete data", () => {
    expect(parseWorkspaceSnapshot("not json")).toEqual({ currentDirectories: {} });
    expect(parseWorkspaceSnapshot('{"currentDirectories":[]}')).toEqual({ currentDirectories: {} });
  });

  it("persists the focus-area button state", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
    };
    expect(loadFocusAreasVisible(storage)).toBe(false);
    saveFocusAreasVisible(true, storage);
    expect(loadFocusAreasVisible(storage)).toBe(true);
    saveFocusAreasVisible(false, storage);
    expect(loadFocusAreasVisible(storage)).toBe(false);
  });
});
