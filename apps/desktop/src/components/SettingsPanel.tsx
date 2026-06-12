import { Check, X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { getHeifCapabilities, getHeifDiagnostics } from "../lib/api";
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
  const {
    hardwareAcceleration,
    locale,
    setHardwareAcceleration,
    setLocale,
    toggleSettings,
  } = useWorkspaceStore();
  const capabilities = useQuery({
    queryKey: ["heif-capabilities"],
    queryFn: getHeifCapabilities,
    staleTime: Infinity,
  });
  const diagnostics = useQuery({
    queryKey: ["heif-diagnostics"],
    queryFn: getHeifDiagnostics,
  });

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
          <span className="settings-panel__label">{t("hardwareAcceleration")}</span>
          <div className="settings-panel__options">
            {[
              { enabled: true, label: t("automatic") },
              { enabled: false, label: t("disabled") },
            ].map(({ enabled, label }) => (
              <button
                key={String(enabled)}
                className={`settings-panel__option ${hardwareAcceleration === enabled ? "is-active" : ""}`}
                onClick={() => setHardwareAcceleration(enabled)}
              >
                <span>{label}</span>
                {hardwareAcceleration === enabled ? <Check size={12} /> : null}
              </button>
            ))}
          </div>
          <div className="settings-panel__diagnostics">
            <small>{t("heifDiagnostics")}</small>
            {capabilities.data?.map((capability) => (
              <span key={capability.backend}>
                {capability.backend}: {capability.available ? capability.acceleration : t("unavailable")}
              </span>
            ))}
            {diagnostics.data ? (
              <span>
                {diagnostics.data.backend} · {diagnostics.data.totalMs} ms
                {diagnostics.data.fallbackReason ? ` · ${diagnostics.data.fallbackReason}` : ""}
              </span>
            ) : null}
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
