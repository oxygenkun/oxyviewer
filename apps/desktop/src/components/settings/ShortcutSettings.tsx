import { RotateCcw } from "lucide-react";
import { useEffect, useState } from "react";
import type { MessageKey } from "@/lib/i18n";
import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_ACTIONS,
  findShortcutConflict,
  formatShortcut,
  shortcutFromEvent,
  shortcutSlotsEqual,
  type ShortcutAction,
  type ShortcutSlot,
} from "@/lib/ui/shortcuts";
import { useWorkspaceStore } from "@/store";

const ACTION_LABELS: Record<ShortcutAction, MessageKey> = {
  "loupe.previousAsset": "shortcutPreviousAsset",
  "loupe.nextAsset": "shortcutNextAsset",
  "loupe.toggleFocusAreas": "shortcutToggleFocusAreas",
  "loupe.cycleZoom": "shortcutCycleZoom",
  "grid.moveLeft": "shortcutGridMoveLeft",
  "grid.moveRight": "shortcutGridMoveRight",
  "grid.moveUp": "shortcutGridMoveUp",
  "grid.moveDown": "shortcutGridMoveDown",
  "grid.openLoupe": "shortcutOpenLoupe",
  "marking.rating1": "shortcutRating1",
  "marking.rating2": "shortcutRating2",
  "marking.rating3": "shortcutRating3",
  "marking.rating4": "shortcutRating4",
  "marking.rating5": "shortcutRating5",
  "marking.clearRating": "shortcutClearRating",
  "marking.colorRed": "shortcutColorRed",
  "marking.colorYellow": "shortcutColorYellow",
  "marking.colorGreen": "shortcutColorGreen",
  "marking.colorBlue": "shortcutColorBlue",
};

const ACTION_GROUPS: Array<{ labelKey: MessageKey; actions: ShortcutAction[] }> = [
  { labelKey: "shortcutGroupLoupe", actions: ["loupe.previousAsset", "loupe.nextAsset", "loupe.toggleFocusAreas", "loupe.cycleZoom"] },
  { labelKey: "shortcutGroupGrid", actions: ["grid.moveLeft", "grid.moveRight", "grid.moveUp", "grid.moveDown", "grid.openLoupe"] },
  {
    labelKey: "shortcutGroupMarking",
    actions: [
      "marking.rating1", "marking.rating2", "marking.rating3", "marking.rating4", "marking.rating5",
      "marking.clearRating",
      "marking.colorRed", "marking.colorYellow", "marking.colorGreen", "marking.colorBlue",
    ],
  },
];

const CLEAR_KEYS = new Set(["Backspace", "Delete"]);

interface Capture {
  action: ShortcutAction;
  slot: ShortcutSlot;
}

export function ShortcutSettings({ t }: { t: (key: MessageKey) => string }) {
  const shortcuts = useWorkspaceStore((state) => state.shortcuts);
  const setShortcut = useWorkspaceStore((state) => state.setShortcut);
  const resetShortcut = useWorkspaceStore((state) => state.resetShortcut);
  const resetShortcuts = useWorkspaceStore((state) => state.resetShortcuts);
  const [capturing, setCapturing] = useState<Capture>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    if (!capturing) return;
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      const { action, slot } = capturing;
      setCapturing(undefined);
      if (event.key === "Escape") return;
      if (CLEAR_KEYS.has(event.key)) {
        setError(undefined);
        setShortcut(action, slot, null);
        return;
      }
      const binding = shortcutFromEvent(event);
      if (!binding) return;
      const conflict = findShortcutConflict(shortcuts, action, slot, binding);
      if (conflict) {
        setError(conflict === action
          ? t("shortcutDuplicate")
          : t("shortcutConflict").replace("{action}", t(ACTION_LABELS[conflict])));
      } else {
        setError(undefined);
        setShortcut(action, slot, binding);
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [capturing, shortcuts, setShortcut, t]);

  const customized = SHORTCUT_ACTIONS.some(
    (action) => !shortcutSlotsEqual(shortcuts[action], DEFAULT_SHORTCUTS[action]),
  );

  return (
    <section className="settings-panel__section shortcut-settings">
      <div className="settings-panel__section-heading">
        <span className="settings-panel__label">{t("shortcutActions")}</span>
        <button disabled={!customized} onClick={() => { setError(undefined); resetShortcuts(); }}>
          <RotateCcw size={14} /> {t("shortcutResetAll")}
        </button>
      </div>
      <p className="settings-panel__hint">{t("shortcutHint")}</p>
      {ACTION_GROUPS.map((group) => (
        <div className="shortcut-settings__group" key={group.labelKey}>
          <h3 className="shortcut-settings__group-title">{t(group.labelKey)}</h3>
          {group.actions.map((action) => {
            const bindingSlots = shortcuts[action];
            const isDefault = shortcutSlotsEqual(bindingSlots, DEFAULT_SHORTCUTS[action]);
            const label = t(ACTION_LABELS[action]);
            return (
              <div className="shortcut-settings__row" key={action}>
                <span className="shortcut-settings__action">{label}</span>
                {bindingSlots.map((binding, slot) => {
                  const isCapturing = capturing?.action === action && capturing.slot === slot;
                  return (
                    <button
                      key={slot}
                      type="button"
                      aria-label={`${label} · ${t("shortcutSlotLabel").replace("{index}", String(slot + 1))}`}
                      aria-pressed={isCapturing}
                      title={t("shortcutEditHint")}
                      className={["shortcut-settings__keys", isCapturing && "is-listening", !binding && "is-unset"]
                        .filter(Boolean)
                        .join(" ")}
                      onClick={() => {
                        setError(undefined);
                        setCapturing(isCapturing ? undefined : { action, slot: slot as ShortcutSlot });
                      }}
                    >
                      {isCapturing ? t("shortcutListening") : binding ? formatShortcut(binding) : t("shortcutUnset")}
                    </button>
                  );
                })}
                <div className="shortcut-settings__actions">
                  <button
                    aria-label={`${t("restoreDefault")} ${label}`}
                    disabled={isDefault}
                    onClick={() => { setError(undefined); resetShortcut(action); }}
                  >
                    {t("restoreDefault")}
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      ))}
      {error ? <p role="alert" className="settings-panel__error">{error}</p> : null}
    </section>
  );
}
