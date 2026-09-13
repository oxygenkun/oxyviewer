import {
  Check,
  Database,
  ExternalLink,
  FolderOpen,
  HardDrive,
  Keyboard,
  LoaderCircle,
  Monitor,
  RotateCcw,
  SlidersHorizontal,
  Trash2,
  X,
} from "lucide-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import {
  chooseCacheParent,
  clearPreviewCache,
  getCacheSettings,
  getHeifCapabilities,
  updateCacheSettings,
} from "../lib/api";
import { clearImageProjections } from "../lib/imageProjection";
import {
  GIB,
  MAX_CACHE_GB,
  MIN_CACHE_GB,
  cacheLimitGb,
  formatBytes,
  validCacheLimitGb,
} from "../lib/cacheSettings";
import type { Locale, MessageKey } from "../lib/i18n";
import { UI_FONT_SCALES } from "../lib/workspacePersistence";
import { useWorkspaceStore } from "../store";
import { RawDecoderPanel } from "./RawDecoderPanel";
import { ExternalAppsSettings } from "./ExternalAppsSettings";
import { ShortcutSettings } from "./ShortcutSettings";
import type { AssetSummary } from "../types";

interface SettingsPanelProps {
  t: (key: MessageKey) => string;
  activeAsset?: AssetSummary;
}

const languages: { value: Locale; label: string }[] = [
  { value: "zh-CN", label: "中文" },
  { value: "en", label: "English" },
];

export function SettingsPanel({ t, activeAsset }: SettingsPanelProps) {
  const queryClient = useQueryClient();
  const [limitGb, setLimitGb] = useState(10);
  const [clearArmed, setClearArmed] = useState(false);
  const {
    displaySharpening,
    gridMetadataVisible,
    locale,
    loupeMetadataVisible,
    uiFontScale,
    setDisplaySharpening,
    setGridMetadataVisible,
    setLocale,
    setLoupeMetadataVisible,
    setSettingsSection,
    setUiFontScale,
    toggleSettings,
    settingsSection,
  } = useWorkspaceStore();
  const capabilities = useQuery({
    queryKey: ["heif-capabilities"],
    queryFn: getHeifCapabilities,
    staleTime: Infinity,
  });
  const cacheSettings = useQuery({
    queryKey: ["cache-settings"],
    queryFn: getCacheSettings,
  });
  const updateCache = useMutation({
    mutationFn: ({ customParent, maxSizeBytes }: { customParent: string | null; maxSizeBytes: number }) =>
      updateCacheSettings(customParent, maxSizeBytes),
    onSuccess: (settings) => queryClient.setQueryData(["cache-settings"], settings),
  });
  const clearCache = useMutation({
    mutationFn: clearPreviewCache,
    onSuccess: (settings) => {
      setClearArmed(false);
      clearImageProjections();
      queryClient.removeQueries({ queryKey: ["asset-render"] });
      queryClient.setQueryData(["cache-settings"], settings);
    },
  });

  useEffect(() => {
    if (cacheSettings.data) setLimitGb(cacheLimitGb(cacheSettings.data.maxSizeBytes));
  }, [cacheSettings.data]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") toggleSettings();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [toggleSettings]);

  useEffect(() => {
    if (!clearArmed) return;
    const timer = window.setTimeout(() => setClearArmed(false), 10_000);
    return () => window.clearTimeout(timer);
  }, [clearArmed]);

  const settings = cacheSettings.data;
  const limitIsValid = validCacheLimitGb(limitGb);
  const usagePercent = settings
    ? Math.min(100, (settings.usedSizeBytes / settings.maxSizeBytes) * 100)
    : 0;
  const cacheBusy = updateCache.isPending || clearCache.isPending;
  const cacheError = cacheSettings.error ?? updateCache.error ?? clearCache.error;

  const chooseLocation = async () => {
    const customParent = await chooseCacheParent();
    if (customParent && settings) {
      updateCache.mutate({ customParent, maxSizeBytes: settings.maxSizeBytes });
    }
  };

  const tabs = [
    { id: "general" as const, label: t("settingsGeneral"), description: t("settingsGeneralDescription"), icon: <SlidersHorizontal size={16} /> },
    { id: "display" as const, label: t("settingsDisplay"), description: t("settingsDisplayDescription"), icon: <Monitor size={16} /> },
    { id: "media" as const, label: t("settingsMedia"), description: t("settingsMediaDescription"), icon: <Database size={16} /> },
    { id: "externalApps" as const, label: t("settingsExternal"), description: t("settingsExternalDescription"), icon: <ExternalLink size={16} /> },
    { id: "shortcuts" as const, label: t("settingsShortcuts"), description: t("settingsShortcutsDescription"), icon: <Keyboard size={16} /> },
  ];
  const activeTab = tabs.find((tab) => tab.id === settingsSection) ?? tabs[0];

  return (
    <div className="settings-overlay" onClick={toggleSettings}>
      <div
        aria-labelledby="settings-title"
        aria-modal="true"
        className="settings-panel"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
      >
        <header className="settings-panel__header">
          <div>
            <span id="settings-title">{t("settings")}</span>
            <small>{t("settingsSubtitle")}</small>
          </div>
          <button aria-label={t("closeSettings")} onClick={toggleSettings}><X size={18} /></button>
        </header>
        <div className="settings-panel__body">
          <nav aria-label={t("settings")} className="settings-panel__tabs" role="tablist">
            {tabs.map((tab) => (
              <button
                aria-controls="settings-tabpanel"
                aria-selected={settingsSection === tab.id}
                className={settingsSection === tab.id ? "is-active" : ""}
                id={`settings-tab-${tab.id}`}
                key={tab.id}
                onClick={() => setSettingsSection(tab.id)}
                role="tab"
              >
                {tab.icon}
                <span>{tab.label}<small>{tab.description}</small></span>
              </button>
            ))}
          </nav>

          <div
            aria-labelledby={`settings-tab-${activeTab.id}`}
            className="settings-panel__content"
            id="settings-tabpanel"
            role="tabpanel"
          >
            <div className="settings-panel__content-heading">
              <h2>{activeTab.label}</h2>
              <p>{activeTab.description}</p>
            </div>

            {settingsSection === "general" ? <>
              <section className="settings-panel__section">
                <span className="settings-panel__label">{t("language")}</span>
                <div className="settings-panel__options">
                  {languages.map(({ value, label }) => (
                    <button key={value} className={`settings-panel__option ${locale === value ? "is-active" : ""}`} onClick={() => setLocale(value)}>
                      <span>{label}</span>{locale === value ? <Check size={14} /> : null}
                    </button>
                  ))}
                </div>
              </section>
              <section className="settings-panel__section">
                <span className="settings-panel__label">{t("appearance")}</span>
                <div className="settings-panel__field">
                  <span>{t("uiFontSize")}</span>
                  <div className="settings-panel__font-size-options">
                    {UI_FONT_SCALES.map((scale) => (
                      <button key={scale} aria-label={`${t("uiFontSize")} ${Math.round(scale * 100)}%`} aria-pressed={uiFontScale === scale} className={uiFontScale === scale ? "is-active" : ""} onClick={() => setUiFontScale(scale)}>
                        {Math.round(scale * 100)}%
                      </button>
                    ))}
                  </div>
                </div>
                <p className="settings-panel__scale-hint">{t("uiFontSizeHint")}</p>
                <div className="settings-panel__theme-row">
                  <span className="settings-panel__theme-swatch settings-panel__theme-swatch--dark" />
                  <span>{t("theme")}</span><small>Dark</small>
                </div>
              </section>
            </> : null}

            {settingsSection === "display" ? <>
              <section className="settings-panel__section">
                <span className="settings-panel__label">{t("metadataDisplay")}</span>
                <div className="settings-panel__options">
                  {[
                    { enabled: gridMetadataVisible, label: t("showGridMetadata"), toggle: () => setGridMetadataVisible(!gridMetadataVisible) },
                    { enabled: loupeMetadataVisible, label: t("showLoupeMetadata"), toggle: () => setLoupeMetadataVisible(!loupeMetadataVisible) },
                  ].map(({ enabled, label, toggle }) => (
                    <button key={label} aria-pressed={enabled} className={`settings-panel__option ${enabled ? "is-active" : ""}`} onClick={toggle}>
                      <span>{label}</span>{enabled ? <Check size={14} /> : null}
                    </button>
                  ))}
                </div>
              </section>
              <section className="settings-panel__section">
                <span className="settings-panel__label">{t("displaySharpening")}</span>
                <div className="settings-panel__options">
                  {[{ enabled: true, label: t("standard") }, { enabled: false, label: t("disabled") }].map(({ enabled, label }) => (
                    <button key={String(enabled)} aria-pressed={displaySharpening === enabled} className={`settings-panel__option ${displaySharpening === enabled ? "is-active" : ""}`} onClick={() => setDisplaySharpening(enabled)}>
                      <span>{label}</span>{displaySharpening === enabled ? <Check size={14} /> : null}
                    </button>
                  ))}
                </div>
              </section>
            </> : null}

            {settingsSection === "media" ? <>
              <section className="settings-panel__section settings-panel__section--storage">
                <div className="settings-panel__section-heading">
                  <span className="settings-panel__label">{t("previewCache")}</span>
                  {settings ? <span className="settings-panel__storage-total">{formatBytes(settings.usedSizeBytes)} / {formatBytes(settings.maxSizeBytes)}</span> : null}
                </div>
                <div className="settings-panel__meter" aria-hidden="true"><span style={{ width: `${usagePercent}%` }} /></div>
                <div className="settings-panel__cache-location">
                  <HardDrive size={16} />
                  <div><span>{settings?.isCustomLocation ? t("customLocation") : t("systemDefault")}</span><code title={settings?.location}>{settings?.location ?? t("loading")}</code></div>
                </div>
                <div className="settings-panel__cache-actions">
                  <button disabled={!settings || cacheBusy} onClick={chooseLocation}><FolderOpen size={14} /> {t("chooseLocation")}</button>
                  <button disabled={!settings?.isCustomLocation || cacheBusy} onClick={() => settings && updateCache.mutate({ customParent: null, maxSizeBytes: settings.maxSizeBytes })}><RotateCcw size={14} /> {t("restoreDefault")}</button>
                </div>
                <p className="settings-panel__hint">{t("cacheLocationHint")}</p>
                <div className="settings-panel__limit-row">
                  <label htmlFor="cache-limit">{t("cacheLimit")}</label>
                  <div className="settings-panel__limit-input"><input aria-invalid={!limitIsValid} id="cache-limit" max={MAX_CACHE_GB} min={MIN_CACHE_GB} onChange={(event) => setLimitGb(Number(event.target.value))} type="number" value={limitGb} /><span>GB</span></div>
                  <button disabled={!settings || !limitIsValid || cacheBusy || limitGb === cacheLimitGb(settings.maxSizeBytes)} onClick={() => settings && updateCache.mutate({ customParent: settings.customParent ?? null, maxSizeBytes: limitGb * GIB })}>{t("apply")}</button>
                </div>
                <div className="settings-panel__cache-footer">
                  <span>{t("cacheLimitRange")}</span>
                  <button className={clearArmed ? "is-armed" : ""} disabled={!settings || cacheBusy} onClick={() => clearArmed ? clearCache.mutate() : setClearArmed(true)}>
                    {clearCache.isPending ? <LoaderCircle className="is-spinning" size={14} /> : <Trash2 size={14} />}{clearArmed ? t("confirmClearCache") : t("clearCache")}
                  </button>
                </div>
                {cacheError ? <p className="settings-panel__error">{String(cacheError)}</p> : null}
              </section>
              <section className="settings-panel__section"><RawDecoderPanel t={t} asset={activeAsset?.kind === "raw" ? activeAsset : undefined} /></section>
              <section className="settings-panel__section">
                <div className="settings-panel__diagnostics"><small>{t("heifDiagnostics")}</small>{capabilities.data?.map((capability) => <span key={capability.backend}>{capability.backend}: {capability.available ? capability.acceleration : t("unavailable")}</span>)}</div>
              </section>
            </> : null}

            {settingsSection === "externalApps" ? <ExternalAppsSettings t={t} focus /> : null}

            {settingsSection === "shortcuts" ? <ShortcutSettings t={t} /> : null}
          </div>
        </div>
      </div>
    </div>
  );
}
