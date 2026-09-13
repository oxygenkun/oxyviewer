import { describe, expect, it } from "vitest";
import {
  DEFAULT_SHORTCUTS,
  findShortcutConflict,
  formatShortcut,
  loadShortcuts,
  matchesAction,
  matchesShortcut,
  saveShortcuts,
  shortcutFromEvent,
  type ShortcutBindings,
} from "./shortcuts";

function memoryStorage(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => { data.set(key, value); },
  };
}

function keyEvent(key: string, modifiers: Partial<Pick<KeyboardEvent, "ctrlKey" | "altKey" | "shiftKey" | "metaKey">> = {}) {
  return {
    key,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    ...modifiers,
  } as KeyboardEvent;
}

describe("loadShortcuts", () => {
  it("returns defaults when nothing is stored", () => {
    expect(loadShortcuts(memoryStorage())).toEqual(DEFAULT_SHORTCUTS);
  });

  it("merges stored overrides over defaults", () => {
    const storage = memoryStorage();
    const custom: ShortcutBindings = {
      ...DEFAULT_SHORTCUTS,
      "loupe.previousAsset": { key: "k", ctrl: false, alt: false, shift: false, meta: false },
    };
    saveShortcuts(custom, storage);
    const loaded = loadShortcuts(storage);
    expect(loaded["loupe.previousAsset"].key).toBe("k");
    expect(loaded["loupe.nextAsset"]).toEqual(DEFAULT_SHORTCUTS["loupe.nextAsset"]);
  });

  it("falls back to defaults for malformed entries", () => {
    const storage = memoryStorage({
      "oxyviewer.shortcuts.v1": JSON.stringify({
        "loupe.previousAsset": { key: 42 },
        "loupe.nextAsset": null,
      }),
    });
    const loaded = loadShortcuts(storage);
    expect(loaded).toEqual(DEFAULT_SHORTCUTS);
  });

  it("returns defaults for invalid JSON", () => {
    expect(loadShortcuts(memoryStorage({ "oxyviewer.shortcuts.v1": "{oops" }))).toEqual(DEFAULT_SHORTCUTS);
  });
});

describe("matchesShortcut", () => {
  it("matches the default arrow bindings", () => {
    expect(matchesShortcut(keyEvent("ArrowLeft"), DEFAULT_SHORTCUTS["loupe.previousAsset"])).toBe(true);
    expect(matchesShortcut(keyEvent("ArrowRight"), DEFAULT_SHORTCUTS["loupe.nextAsset"])).toBe(true);
  });

  it("rejects extra modifiers", () => {
    expect(matchesShortcut(keyEvent("ArrowLeft", { shiftKey: true }), DEFAULT_SHORTCUTS["loupe.previousAsset"])).toBe(false);
    expect(matchesShortcut(keyEvent("f", { ctrlKey: true }), DEFAULT_SHORTCUTS["loupe.toggleFocusAreas"])).toBe(false);
  });

  it("matches letter keys case-insensitively", () => {
    expect(matchesShortcut(keyEvent("F"), DEFAULT_SHORTCUTS["loupe.toggleFocusAreas"])).toBe(true);
  });
});

describe("shortcutFromEvent", () => {
  it("captures key and modifiers", () => {
    expect(shortcutFromEvent(keyEvent("J", { shiftKey: true }))).toEqual({
      key: "j", ctrl: false, alt: false, shift: true, meta: false,
    });
  });

  it("ignores pure modifier presses", () => {
    expect(shortcutFromEvent(keyEvent("Shift", { shiftKey: true }))).toBeUndefined();
    expect(shortcutFromEvent(keyEvent("Control", { ctrlKey: true }))).toBeUndefined();
  });
});

describe("matchesAction", () => {
  it("matches built-in aliases in addition to the customizable binding", () => {
    expect(matchesAction(keyEvent("`"), DEFAULT_SHORTCUTS, "marking.clearRating")).toBe(true);
    expect(matchesAction(keyEvent("0"), DEFAULT_SHORTCUTS, "marking.clearRating")).toBe(true);
    expect(matchesAction(keyEvent("`"), DEFAULT_SHORTCUTS, "marking.rating1")).toBe(false);
  });
});

describe("findShortcutConflict", () => {
  it("finds another action with the same binding", () => {
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "loupe.previousAsset",
      DEFAULT_SHORTCUTS["loupe.nextAsset"],
    )).toBe("loupe.nextAsset");
  });

  it("ignores the action's own binding", () => {
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "loupe.previousAsset",
      DEFAULT_SHORTCUTS["loupe.previousAsset"],
    )).toBeUndefined();
  });

  it("allows grid and loupe actions to share bindings", () => {
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "grid.moveLeft",
      DEFAULT_SHORTCUTS["loupe.previousAsset"],
    )).toBeUndefined();
  });

  it("treats marking actions as conflicting with every scope", () => {
    // ArrowLeft is shared by grid and loupe; either may be reported first.
    expect(["grid.moveLeft", "loupe.previousAsset"]).toContain(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "marking.rating1",
      DEFAULT_SHORTCUTS["grid.moveLeft"],
    ));
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "grid.moveLeft",
      DEFAULT_SHORTCUTS["marking.rating1"],
    )).toBe("marking.rating1");
  });

  it("detects conflicts with built-in aliases", () => {
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "marking.rating1",
      { key: "`", ctrl: false, alt: false, shift: false, meta: false },
    )).toBe("marking.clearRating");
  });
});

describe("formatShortcut", () => {
  it("formats arrows and modifiers", () => {
    expect(formatShortcut(DEFAULT_SHORTCUTS["loupe.previousAsset"])).toBe("←");
    expect(formatShortcut({ key: "p", ctrl: true, alt: false, shift: true, meta: false })).toBe("Ctrl+Shift+P");
  });
});
