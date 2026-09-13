const SHORTCUTS_KEY = "oxyviewer.shortcuts.v1";

export interface ShortcutBinding {
  key: string;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
}

export const SHORTCUT_ACTIONS = [
  "loupe.previousAsset",
  "loupe.nextAsset",
  "loupe.toggleFocusAreas",
  "loupe.cycleZoom",
  "grid.moveLeft",
  "grid.moveRight",
  "grid.moveUp",
  "grid.moveDown",
  "grid.openLoupe",
  "marking.rating1",
  "marking.rating2",
  "marking.rating3",
  "marking.rating4",
  "marking.rating5",
  "marking.clearRating",
  "marking.colorRed",
  "marking.colorYellow",
  "marking.colorGreen",
  "marking.colorBlue",
] as const;

export type ShortcutAction = (typeof SHORTCUT_ACTIONS)[number];
export type ShortcutBindings = Record<ShortcutAction, ShortcutBinding>;

const plain = (key: string): ShortcutBinding => ({ key, ctrl: false, alt: false, shift: false, meta: false });

export const DEFAULT_SHORTCUTS: ShortcutBindings = {
  "loupe.previousAsset": plain("arrowleft"),
  "loupe.nextAsset": plain("arrowright"),
  "loupe.toggleFocusAreas": plain("f"),
  "loupe.cycleZoom": plain("space"),
  "grid.moveLeft": plain("arrowleft"),
  "grid.moveRight": plain("arrowright"),
  "grid.moveUp": plain("arrowup"),
  "grid.moveDown": plain("arrowdown"),
  "grid.openLoupe": plain("space"),
  "marking.rating1": plain("1"),
  "marking.rating2": plain("2"),
  "marking.rating3": plain("3"),
  "marking.rating4": plain("4"),
  "marking.rating5": plain("5"),
  "marking.clearRating": plain("0"),
  "marking.colorRed": plain("6"),
  "marking.colorYellow": plain("7"),
  "marking.colorGreen": plain("8"),
  "marking.colorBlue": plain("9"),
};

// Extra built-in bindings that always match alongside the customizable one.
export const SHORTCUT_ALIASES: Partial<Record<ShortcutAction, ShortcutBinding[]>> = {
  "marking.clearRating": [plain("`")],
};

export type ShortcutScope = "loupe" | "grid" | "marking";

export const SHORTCUT_SCOPES: Record<ShortcutAction, ShortcutScope> = {
  "loupe.previousAsset": "loupe",
  "loupe.nextAsset": "loupe",
  "loupe.toggleFocusAreas": "loupe",
  "loupe.cycleZoom": "loupe",
  "grid.moveLeft": "grid",
  "grid.moveRight": "grid",
  "grid.moveUp": "grid",
  "grid.moveDown": "grid",
  "grid.openLoupe": "grid",
  "marking.rating1": "marking",
  "marking.rating2": "marking",
  "marking.rating3": "marking",
  "marking.rating4": "marking",
  "marking.rating5": "marking",
  "marking.clearRating": "marking",
  "marking.colorRed": "marking",
  "marking.colorYellow": "marking",
  "marking.colorGreen": "marking",
  "marking.colorBlue": "marking",
};

// Grid and loupe are never active at the same time, so their bindings may
// overlap. Marking shortcuts fire in both views and conflict with everything.
function scopesConflict(a: ShortcutScope, b: ShortcutScope): boolean {
  return a === b || a === "marking" || b === "marking";
}

const MODIFIER_KEYS = new Set(["control", "alt", "shift", "meta"]);

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function normalizeKey(key: string): string {
  return key === " " ? "space" : key.toLowerCase();
}

function isBinding(value: unknown): value is ShortcutBinding {
  if (!value || typeof value !== "object") return false;
  const binding = value as Partial<ShortcutBinding>;
  return typeof binding.key === "string"
    && binding.key.length > 0
    && typeof binding.ctrl === "boolean"
    && typeof binding.alt === "boolean"
    && typeof binding.shift === "boolean"
    && typeof binding.meta === "boolean";
}

export function loadShortcuts(storage?: StorageLike): ShortcutBindings {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return { ...DEFAULT_SHORTCUTS };
  try {
    const parsed = JSON.parse(resolved.getItem(SHORTCUTS_KEY) ?? "{}") as Partial<Record<ShortcutAction, unknown>>;
    return Object.fromEntries(
      SHORTCUT_ACTIONS.map((action) => {
        const stored = parsed[action];
        return [action, isBinding(stored) ? { ...stored, key: normalizeKey(stored.key) } : { ...DEFAULT_SHORTCUTS[action] }];
      }),
    ) as ShortcutBindings;
  } catch {
    return { ...DEFAULT_SHORTCUTS };
  }
}

export function saveShortcuts(bindings: ShortcutBindings, storage?: StorageLike): void {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return;
  try {
    resolved.setItem(SHORTCUTS_KEY, JSON.stringify(bindings));
  } catch {
    // Shortcut preferences must never prevent the viewer from opening.
  }
}

export function bindingsEqual(a: ShortcutBinding, b: ShortcutBinding): boolean {
  return a.key === b.key
    && a.ctrl === b.ctrl
    && a.alt === b.alt
    && a.shift === b.shift
    && a.meta === b.meta;
}

export function matchesShortcut(event: KeyboardEvent, binding: ShortcutBinding): boolean {
  return normalizeKey(event.key) === binding.key
    && event.ctrlKey === binding.ctrl
    && event.altKey === binding.alt
    && event.shiftKey === binding.shift
    && event.metaKey === binding.meta;
}

export function shortcutFromEvent(event: KeyboardEvent): ShortcutBinding | undefined {
  const key = normalizeKey(event.key);
  if (MODIFIER_KEYS.has(key)) return undefined;
  return { key, ctrl: event.ctrlKey, alt: event.altKey, shift: event.shiftKey, meta: event.metaKey };
}

export function matchesAction(
  event: KeyboardEvent,
  bindings: ShortcutBindings,
  action: ShortcutAction,
): boolean {
  return matchesShortcut(event, bindings[action])
    || (SHORTCUT_ALIASES[action] ?? []).some((alias) => matchesShortcut(event, alias));
}

export function findShortcutConflict(
  bindings: ShortcutBindings,
  action: ShortcutAction,
  binding: ShortcutBinding,
): ShortcutAction | undefined {
  return SHORTCUT_ACTIONS.find(
    (candidate) => candidate !== action
      && scopesConflict(SHORTCUT_SCOPES[action], SHORTCUT_SCOPES[candidate])
      && (bindingsEqual(bindings[candidate], binding)
        || (SHORTCUT_ALIASES[candidate] ?? []).some((alias) => bindingsEqual(alias, binding))),
  );
}

const KEY_LABELS: Record<string, string> = {
  arrowleft: "←",
  arrowright: "→",
  arrowup: "↑",
  arrowdown: "↓",
  space: "Space",
  escape: "Esc",
  enter: "Enter",
  backspace: "Backspace",
  delete: "Delete",
  tab: "Tab",
  home: "Home",
  end: "End",
  pageup: "PageUp",
  pagedown: "PageDown",
};

export function formatShortcut(binding: ShortcutBinding): string {
  const parts: string[] = [];
  if (binding.ctrl) parts.push("Ctrl");
  if (binding.alt) parts.push("Alt");
  if (binding.shift) parts.push("Shift");
  if (binding.meta) parts.push("Meta");
  const label = KEY_LABELS[binding.key];
  parts.push(label ?? (binding.key.length === 1 ? binding.key.toUpperCase() : binding.key));
  return parts.join("+");
}
