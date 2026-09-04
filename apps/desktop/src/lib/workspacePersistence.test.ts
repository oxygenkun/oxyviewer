import { describe, expect, it } from "vitest";
import {
  loadFocusAreasVisible,
  loadLoupeControlsAutoHide,
  loadLayoutSize,
  loadMetadataVisibility,
  loadUiFontScale,
  parseWorkspaceSnapshot,
  recoverMissingCurrentDirectory,
  saveFocusAreasVisible,
  saveLoupeControlsAutoHide,
  saveLayoutSize,
  saveMetadataVisibility,
  saveUiFontScale,
} from "./workspacePersistence";

describe("workspace persistence", () => {
  it("restores the active root and each root's last directory", () => {
    expect(parseWorkspaceSnapshot(JSON.stringify({
      activeRoot: "/photos",
      folderSort: "nameDescending",
      folderDragEnabled: true,
      currentDirectories: { "/photos": "/photos/2025", "/archive": "/archive" },
    }))).toEqual({
      activeRoot: "/photos",
      folderSort: "nameDescending",
      folderDragEnabled: true,
      currentDirectories: { "/photos": "/photos/2025", "/archive": "/archive" },
    });
  });

  it("fails closed for malformed or obsolete data", () => {
    expect(parseWorkspaceSnapshot("not json")).toEqual({ currentDirectories: {} });
    expect(parseWorkspaceSnapshot('{"currentDirectories":[]}')).toEqual({ currentDirectories: {} });
  });

  it("falls back to the root when a restored current directory disappeared", () => {
    const snapshot = {
      activeRoot: "/photos",
      currentDirectories: { "/photos": "/photos/renamed", "/archive": "/archive/2025" },
    };

    expect(recoverMissingCurrentDirectory(snapshot, "/photos", "/photos/renamed")).toEqual({
      activeRoot: "/photos",
      currentDirectories: { "/photos": "/photos", "/archive": "/archive/2025" },
    });
    expect(recoverMissingCurrentDirectory(snapshot, "/photos", "/photos/older")).toBe(snapshot);
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

  it("persists independent grid and loupe metadata visibility", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
    };
    expect(loadMetadataVisibility("grid", storage)).toBe(true);
    expect(loadMetadataVisibility("loupe", storage)).toBe(true);
    saveMetadataVisibility("grid", false, storage);
    expect(loadMetadataVisibility("grid", storage)).toBe(false);
    expect(loadMetadataVisibility("loupe", storage)).toBe(true);
  });

  it("persists loupe toolbar auto-hide", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
    };
    expect(loadLoupeControlsAutoHide(storage)).toBe(false);
    saveLoupeControlsAutoHide(true, storage);
    expect(loadLoupeControlsAutoHide(storage)).toBe(true);
    saveLoupeControlsAutoHide(false, storage);
    expect(loadLoupeControlsAutoHide(storage)).toBe(false);
  });

  it("persists a supported UI font scale and rejects obsolete values", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
    };
    expect(loadUiFontScale(storage)).toBe(1);
    saveUiFontScale(1.5, storage);
    expect(loadUiFontScale(storage)).toBe(1.5);
    values.set("oxyviewer.ui-font-scale.v1", "1.15");
    expect(loadUiFontScale(storage)).toBe(1.25);
    values.set("oxyviewer.ui-font-scale.v1", "2");
    expect(loadUiFontScale(storage)).toBe(1.75);
    values.set("oxyviewer.ui-font-scale.v1", "9");
    expect(loadUiFontScale(storage)).toBe(1);
  });

  it("persists and clamps layout sizes", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
    };
    expect(loadLayoutSize("leftPanel", storage)).toBe(224);
    saveLayoutSize("leftPanel", 320, storage);
    expect(loadLayoutSize("leftPanel", storage)).toBe(320);
    saveLayoutSize("filmstrip", 10_000, storage);
    expect(loadLayoutSize("filmstrip", storage)).toBe(300);
  });
});
