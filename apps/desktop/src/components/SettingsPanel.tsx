import { Check, X } from "lucide-react";
import type { Locale, MessageKey } from "../lib/i18n";
import { useWorkspaceStore } from "../store";

interface SettingsPanelProps {
  t: (key: MessageKey) => string;
}

const languages: { value: Locale; label: string }[] = [
  { value: "zh-CN", label: "中文" },
  { value: "en", label: "English" },
];

export function SettingsPanel({ t }: SettingsPanelProps) {
  const { locale, setLocale, toggleSettings } = useWorkspaceStore();

  return (
    <div className="settings-overlay" onClick={toggleSettings}>
      <div className="settings-panel" onClick={(e) => e.stopPropagation()}>
        <header className="settings-panel__header">
          <span>{t("settings")}</span>
          <button onClick={toggleSettings}><X size={14} /></button>
        </header>

        <div className="settings-panel__section">
          <span className="settings-panel__label">{t("language")}</span>
          <div className="settings-panel__options">
            {languages.map(({ value, label }) => (
              <button
                key={value}
                className={`settings-panel__option ${locale === value ? "is-active" : ""}`}
                onClick={() => setLocale(value)}
              >
                <span>{label}</span>
                {locale === value ? <Check size={12} /> : null}
              </button>
            ))}
          </div>
        </div>

        <div className="settings-panel__section">
          <span className="settings-panel__label">{t("appearance")}</span>
          <div className="settings-panel__theme-row">
            <span className="settings-panel__theme-swatch settings-panel__theme-swatch--dark" />
            <span>{t("theme")}</span>
            <small>Dark</small>
          </div>
        </div>
      </div>
    </div>
  );
}
