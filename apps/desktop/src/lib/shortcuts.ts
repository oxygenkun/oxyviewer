const SHORTCUTS_KEY = "oxyviewer.shortcuts.v2";
const LEGACY_SHORTCUTS_KEY = "oxyviewer.shortcuts.v1";

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

// Every action owns two independently customizable slots. A null slot is an
// intentionally unbound key.
export type ShortcutSlot = 0 | 1;
export type ShortcutSlots = [ShortcutBinding | null, ShortcutBinding | null];
export type ShortcutBindings = Record<ShortcutAction, ShortcutSlots>;

const plain = (key: string): ShortcutBinding => ({ key, ctrl: false, alt: false, shift: false, meta: false });
const slots = (primary: ShortcutBinding | null, secondary: ShortcutBinding | null = null): ShortcutSlots =>
  [primary, secondary];

export const DEFAULT_SHORTCUTS: ShortcutBindings = {
  "loupe.previousAsset": slots(plain("arrowleft")),
  "loupe.nextAsset": slots(plain("arrowright")),
  "loupe.toggleFocusAreas": slots(plain("f")),
  "loupe.cycleZoom": slots(plain("space")),
  "grid.moveLeft": slots(plain("arrowleft")),
  "grid.moveRight": slots(plain("arrowright")),
  "grid.moveUp": slots(plain("arrowup")),
  "grid.moveDown": slots(plain("arrowdown")),
  "grid.openLoupe": slots(plain("space")),
  "marking.rating1": slots(plain("1")),
  "marking.rating2": slots(plain("2")),
  "marking.rating3": slots(plain("3")),
  "marking.rating4": slots(plain("4")),
  "marking.rating5": slots(plain("5")),
  "marking.clearRating": slots(plain("0"), plain("`")),
  "marking.colorRed": slots(plain("6")),
  "marking.colorYellow": slots(plain("7")),
  "marking.colorGreen": slots(plain("8")),
  "marking.colorBlue": slots(plain("9")),
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

function cloneSlot(slot: ShortcutBinding | null): ShortcutBinding | null {
  return slot ? { ...slot } : null;
}

export function cloneSlots(source: ShortcutSlots): ShortcutSlots {
  return [cloneSlot(source[0]), cloneSlot(source[1])];
}

export function cloneShortcuts(bindings: ShortcutBindings = DEFAULT_SHORTCUTS): ShortcutBindings {
  return Object.fromEntries(
    SHORTCUT_ACTIONS.map((action) => [action, cloneSlots(bindings[action])]),
  ) as ShortcutBindings;
}

function parseSlots(value: unknown): ShortcutSlots | undefined {
  if (!Array.isArray(value) || value.length !== 2) return undefined;
  const [primary, secondary] = value as unknown[];
  if (primary !== null && !isBinding(primary)) return undefined;
  if (secondary !== null && !isBinding(secondary)) return undefined;
  return [
    isBinding(primary) ? { ...primary, key: normalizeKey(primary.key) } : null,
    isBinding(secondary) ? { ...secondary, key: normalizeKey(secondary.key) } : null,
  ];
}

function parseStored(raw: string | null): Partial<Record<ShortcutAction, unknown>> | undefined {
  if (!raw) return undefined;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return undefined;
    return parsed as Partial<Record<ShortcutAction, unknown>>;
  } catch {
    return undefined;
  }
}

function fromStoredSlots(stored: Partial<Record<ShortcutAction, unknown>>): ShortcutBindings {
  return Object.fromEntries(
    SHORTCUT_ACTIONS.map((action) => [
      action,
      parseSlots(stored[action]) ?? cloneSlots(DEFAULT_SHORTCUTS[action]),
    ]),
  ) as ShortcutBindings;
}

// v1 stored one binding per action. It becomes the first slot, while the second
// slot keeps its default so upgraded users retain every default key.
function fromLegacyBindings(stored: Partial<Record<ShortcutAction, unknown>>): ShortcutBindings {
  return Object.fromEntries(
    SHORTCUT_ACTIONS.map((action) => {
      const entry = stored[action];
      return [
        action,
        isBinding(entry)
          ? [{ ...entry, key: normalizeKey(entry.key) }, cloneSlot(DEFAULT_SHORTCUTS[action][1])]
          : cloneSlots(DEFAULT_SHORTCUTS[action]),
      ];
    }),
  ) as ShortcutBindings;
}

export function loadShortcuts(storage?: StorageLike): ShortcutBindings {
  const resolved = storage ?? (typeof window === "undefined" ? undefined : window.localStorage);
  if (!resolved) return cloneShortcuts();
  const stored = parseStored(resolved.getItem(SHORTCUTS_KEY));
  if (stored) return fromStoredSlots(stored);
  const legacy = parseStored(resolved.getItem(LEGACY_SHORTCUTS_KEY));
  if (legacy) return fromLegacyBindings(legacy);
  return cloneShortcuts();
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
  return bindings[action].some((slot) => slot !== null && matchesShortcut(event, slot));
}

function slotMatches(candidate: ShortcutBinding | null, binding: ShortcutBinding): boolean {
  return candidate !== null && bindingsEqual(candidate, binding);
}

export function shortcutSlotsEqual(a: ShortcutSlots, b: ShortcutSlots): boolean {
  return (a[0] === null || b[0] === null ? a[0] === b[0] : bindingsEqual(a[0], b[0]))
    && (a[1] === null || b[1] === null ? a[1] === b[1] : bindingsEqual(a[1], b[1]));
}

// Returns the action whose binding collides with `binding`, or `action` itself
// when the two slots of one action would hold the same key.
export function findShortcutConflict(
  bindings: ShortcutBindings,
  action: ShortcutAction,
  slot: ShortcutSlot,
  binding: ShortcutBinding,
): ShortcutAction | undefined {
  const conflicting = SHORTCUT_ACTIONS.find(
    (candidate) => candidate !== action
      && scopesConflict(SHORTCUT_SCOPES[action], SHORTCUT_SCOPES[candidate])
      && bindings[candidate].some((candidateSlot) => slotMatches(candidateSlot, binding)),
  );
  if (conflicting) return conflicting;
  return slotMatches(bindings[action][slot === 0 ? 1 : 0], binding) ? action : undefined;
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
  parts.push(label ?? (binding.key.length === 1 ? binding.key.toLowerCase() : binding.key));
  return parts.join("+");
}
