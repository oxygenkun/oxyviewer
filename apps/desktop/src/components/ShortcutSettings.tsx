import { RotateCcw } from "lucide-react";
import { useEffect, useState } from "react";
import type { MessageKey } from "../lib/i18n";
import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_ACTIONS,
  SHORTCUT_ALIASES,
  bindingsEqual,
  findShortcutConflict,
  formatShortcut,
  shortcutFromEvent,
  type ShortcutAction,
} from "../lib/shortcuts";
import { useWorkspaceStore } from "../store";

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

export function ShortcutSettings({ t }: { t: (key: MessageKey) => string }) {
  const shortcuts = useWorkspaceStore((state) => state.shortcuts);
  const setShortcut = useWorkspaceStore((state) => state.setShortcut);
  const resetShortcuts = useWorkspaceStore((state) => state.resetShortcuts);
  const [capturing, setCapturing] = useState<ShortcutAction>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    if (!capturing) return;
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setCapturing(undefined);
        return;
      }
      const binding = shortcutFromEvent(event);
      if (!binding) return;
      const conflict = findShortcutConflict(shortcuts, capturing, binding);
      if (conflict) {
        setError(t("shortcutConflict").replace("{action}", t(ACTION_LABELS[conflict])));
      } else {
        setError(undefined);
        setShortcut(capturing, binding);
      }
      setCapturing(undefined);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [capturing, shortcuts, setShortcut, t]);

  const customized = SHORTCUT_ACTIONS.some(
    (action) => !bindingsEqual(shortcuts[action], DEFAULT_SHORTCUTS[action]),
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
            const binding = shortcuts[action];
            const aliases = SHORTCUT_ALIASES[action] ?? [];
            const isDefault = bindingsEqual(binding, DEFAULT_SHORTCUTS[action]);
            const isCapturing = capturing === action;
            return (
              <div className="shortcut-settings__row" key={action}>
                <span className="shortcut-settings__action">{t(ACTION_LABELS[action])}</span>
                <kbd className={`shortcut-settings__keys ${isCapturing ? "is-listening" : ""}`}>
                  {isCapturing ? t("shortcutListening") : formatShortcut(binding)}
                </kbd>
                {aliases.map((alias) => (
                  <kbd className="shortcut-settings__keys shortcut-settings__keys--alias" key={formatShortcut(alias)}>
                    {formatShortcut(alias)}
                  </kbd>
                ))}
                <div className="shortcut-settings__actions">
                  <button
                    aria-pressed={isCapturing}
                    onClick={() => { setError(undefined); setCapturing(isCapturing ? undefined : action); }}
                  >
                    {isCapturing ? t("cancel") : t("shortcutRebind")}
                  </button>
                  <button
                    aria-label={`${t("restoreDefault")} ${t(ACTION_LABELS[action])}`}
                    disabled={isDefault}
                    onClick={() => { setError(undefined); setShortcut(action, DEFAULT_SHORTCUTS[action]); }}
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
