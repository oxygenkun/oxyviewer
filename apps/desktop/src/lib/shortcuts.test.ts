import { describe, expect, it } from "vitest";
import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_ACTIONS,
  cloneShortcuts,
  findShortcutConflict,
  formatShortcut,
  loadShortcuts,
  matchesAction,
  matchesShortcut,
  saveShortcuts,
  shortcutFromEvent,
  shortcutSlotsEqual,
  type ShortcutAction,
  type ShortcutBinding,
  type ShortcutBindings,
} from "./shortcuts";

const plain = (key: string): ShortcutBinding => ({ key, ctrl: false, alt: false, shift: false, meta: false });

function defaultSlot(action: ShortcutAction, slot: 0 | 1 = 0): ShortcutBinding {
  const binding = DEFAULT_SHORTCUTS[action][slot];
  if (!binding) throw new Error(`${action} slot ${slot} is unbound by default`);
  return binding;
}

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

describe("DEFAULT_SHORTCUTS", () => {
  it("gives every action two slots", () => {
    for (const action of SHORTCUT_ACTIONS) expect(DEFAULT_SHORTCUTS[action]).toHaveLength(2);
    expect(DEFAULT_SHORTCUTS["marking.clearRating"]).toEqual([plain("0"), plain("`")]);
  });
});

describe("loadShortcuts", () => {
  it("returns defaults when nothing is stored", () => {
    expect(loadShortcuts(memoryStorage())).toEqual(DEFAULT_SHORTCUTS);
  });

  it("round-trips customized slots", () => {
    const storage = memoryStorage();
    const custom: ShortcutBindings = {
      ...cloneShortcuts(),
      "loupe.previousAsset": [plain("k"), plain("j")],
      "loupe.toggleFocusAreas": [plain("g"), null],
    };
    saveShortcuts(custom, storage);
    expect(loadShortcuts(storage)).toEqual(custom);
  });

  it("keeps unbound slots unbound", () => {
    const storage = memoryStorage();
    saveShortcuts({ ...cloneShortcuts(), "grid.openLoupe": [null, null] }, storage);
    expect(loadShortcuts(storage)["grid.openLoupe"]).toEqual([null, null]);
  });

  it("migrates legacy single bindings into the first slot", () => {
    const storage = memoryStorage({
      "oxyviewer.shortcuts.v1": JSON.stringify({ "loupe.previousAsset": plain("k") }),
    });
    const loaded = loadShortcuts(storage);
    expect(loaded["loupe.previousAsset"]).toEqual([plain("k"), null]);
    expect(loaded["marking.clearRating"]).toEqual([plain("0"), plain("`")]);
    expect(loaded["loupe.nextAsset"]).toEqual(DEFAULT_SHORTCUTS["loupe.nextAsset"]);
  });

  it("prefers stored slots over legacy bindings", () => {
    const storage = memoryStorage({
      "oxyviewer.shortcuts.v1": JSON.stringify({ "loupe.previousAsset": plain("k") }),
      "oxyviewer.shortcuts.v2": JSON.stringify({ "loupe.previousAsset": [plain("h"), null] }),
    });
    expect(loadShortcuts(storage)["loupe.previousAsset"]).toEqual([plain("h"), null]);
  });

  it("falls back to defaults for malformed entries", () => {
    const storage = memoryStorage({
      "oxyviewer.shortcuts.v2": JSON.stringify({
        "loupe.previousAsset": { key: 42 },
        "loupe.nextAsset": [plain("j")],
        "grid.moveUp": [plain("i"), { key: false }],
      }),
    });
    expect(loadShortcuts(storage)).toEqual(DEFAULT_SHORTCUTS);
  });

  it("returns defaults for invalid JSON", () => {
    expect(loadShortcuts(memoryStorage({ "oxyviewer.shortcuts.v2": "{oops" }))).toEqual(DEFAULT_SHORTCUTS);
    expect(loadShortcuts(memoryStorage({ "oxyviewer.shortcuts.v1": "{oops" }))).toEqual(DEFAULT_SHORTCUTS);
  });
});

describe("matchesShortcut", () => {
  it("matches the default arrow bindings", () => {
    expect(matchesShortcut(keyEvent("ArrowLeft"), defaultSlot("loupe.previousAsset"))).toBe(true);
    expect(matchesShortcut(keyEvent("ArrowRight"), defaultSlot("loupe.nextAsset"))).toBe(true);
  });

  it("rejects extra modifiers", () => {
    expect(matchesShortcut(keyEvent("ArrowLeft", { shiftKey: true }), defaultSlot("loupe.previousAsset"))).toBe(false);
    expect(matchesShortcut(keyEvent("f", { ctrlKey: true }), defaultSlot("loupe.toggleFocusAreas"))).toBe(false);
  });

  it("matches letter keys case-insensitively", () => {
    expect(matchesShortcut(keyEvent("F"), defaultSlot("loupe.toggleFocusAreas"))).toBe(true);
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
  it("matches every customized slot", () => {
    expect(matchesAction(keyEvent("`"), DEFAULT_SHORTCUTS, "marking.clearRating")).toBe(true);
    expect(matchesAction(keyEvent("0"), DEFAULT_SHORTCUTS, "marking.clearRating")).toBe(true);
    expect(matchesAction(keyEvent("`"), DEFAULT_SHORTCUTS, "marking.rating1")).toBe(false);
  });

  it("ignores unbound slots", () => {
    const bindings = cloneShortcuts();
    bindings["grid.openLoupe"] = [null, null];
    expect(matchesAction(keyEvent(" "), bindings, "grid.openLoupe")).toBe(false);
    bindings["grid.openLoupe"] = [null, plain("l")];
    expect(matchesAction(keyEvent("l"), bindings, "grid.openLoupe")).toBe(true);
  });
});

describe("findShortcutConflict", () => {
  it("finds another action with the same binding", () => {
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "loupe.previousAsset",
      0,
      defaultSlot("loupe.nextAsset"),
    )).toBe("loupe.nextAsset");
  });

  it("finds a conflict in another action's second slot", () => {
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "marking.rating1",
      0,
      plain("`"),
    )).toBe("marking.clearRating");
  });

  it("finds a conflict in another action's customized second slot", () => {
    const bindings = cloneShortcuts();
    bindings["marking.rating1"] = [plain("j"), plain("k")];
    expect(findShortcutConflict(bindings, "loupe.nextAsset", 0, plain("k"))).toBe("marking.rating1");
  });

  it("ignores the slot being edited", () => {
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "loupe.previousAsset",
      0,
      defaultSlot("loupe.previousAsset"),
    )).toBeUndefined();
  });

  it("rejects the same key in both slots of one action", () => {
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "marking.clearRating",
      0,
      plain("`"),
    )).toBe("marking.clearRating");
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "marking.clearRating",
      1,
      plain("0"),
    )).toBe("marking.clearRating");
  });

  it("allows grid and loupe actions to share bindings", () => {
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "grid.moveLeft",
      0,
      defaultSlot("loupe.previousAsset"),
    )).toBeUndefined();
  });

  it("treats marking actions as conflicting with every scope", () => {
    // ArrowLeft is shared by grid and loupe; either may be reported first.
    expect(["grid.moveLeft", "loupe.previousAsset"]).toContain(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "marking.rating1",
      0,
      defaultSlot("grid.moveLeft"),
    ));
    expect(findShortcutConflict(
      DEFAULT_SHORTCUTS,
      "grid.moveLeft",
      0,
      defaultSlot("marking.rating1"),
    )).toBe("marking.rating1");
  });
});

describe("shortcutSlotsEqual", () => {
  it("compares bound and unbound slots", () => {
    expect(shortcutSlotsEqual([plain("k"), null], [plain("k"), null])).toBe(true);
    expect(shortcutSlotsEqual([null, null], [null, plain("k")])).toBe(false);
    expect(shortcutSlotsEqual([plain("k"), plain("j")], [plain("k"), plain("k")])).toBe(false);
    expect(shortcutSlotsEqual([plain("k"), null], [plain("K"), null])).toBe(false);
  });
});

describe("formatShortcut", () => {
  it("formats arrows and modifiers", () => {
    expect(formatShortcut(defaultSlot("loupe.previousAsset"))).toBe("←");
    expect(formatShortcut({ key: "p", ctrl: true, alt: false, shift: true, meta: false })).toBe("Ctrl+Shift+p");
    expect(formatShortcut(defaultSlot("loupe.toggleFocusAreas"))).toBe("f");
    expect(formatShortcut({ key: "P", ctrl: false, alt: false, shift: false, meta: false })).toBe("p");
  });
});
