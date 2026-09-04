import { Check, FolderOpen, HardDrive, LoaderCircle, RotateCcw, Trash2, X } from "lucide-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import {
  chooseCacheParent,
  clearPreviewCache,
  getCacheSettings,
  getHeifCapabilities,
  getHeifDiagnostics,
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
import { useWorkspaceStore } from "../store";

interface SettingsPanelProps {
  t: (key: MessageKey) => string;
}

const languages: { value: Locale; label: string }[] = [
  { value: "zh-CN", label: "中文" },
  { value: "en", label: "English" },
];

export function SettingsPanel({ t }: SettingsPanelProps) {
  const queryClient = useQueryClient();
  const [limitGb, setLimitGb] = useState(10);
  const [clearArmed, setClearArmed] = useState(false);
  const {
    displaySharpening,
    gridMetadataVisible,
    hardwareAcceleration,
    locale,
    loupeMetadataVisible,
    setDisplaySharpening,
    setGridMetadataVisible,
    setHardwareAcceleration,
    setLocale,
    setLoupeMetadataVisible,
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
          <span id="settings-title">{t("settings")}</span>
          <button aria-label={t("closeSettings")} onClick={toggleSettings}><X size={14} /></button>
        </header>

        <div className="settings-panel__section settings-panel__section--storage">
          <div className="settings-panel__section-heading">
            <span className="settings-panel__label">{t("previewCache")}</span>
            {settings ? (
              <span className="settings-panel__storage-total">
                {formatBytes(settings.usedSizeBytes)} / {formatBytes(settings.maxSizeBytes)}
              </span>
            ) : null}
          </div>
          <div className="settings-panel__meter" aria-hidden="true">
            <span style={{ width: `${usagePercent}%` }} />
          </div>
          <div className="settings-panel__cache-location">
            <HardDrive size={14} />
            <div>
              <span>{settings?.isCustomLocation ? t("customLocation") : t("systemDefault")}</span>
              <code title={settings?.location}>{settings?.location ?? t("loading")}</code>
            </div>
          </div>
          <div className="settings-panel__cache-actions">
            <button disabled={!settings || cacheBusy} onClick={chooseLocation}>
              <FolderOpen size={12} /> {t("chooseLocation")}
            </button>
            <button
              disabled={!settings?.isCustomLocation || cacheBusy}
              onClick={() => settings && updateCache.mutate({
                customParent: null,
                maxSizeBytes: settings.maxSizeBytes,
              })}
            >
              <RotateCcw size={12} /> {t("restoreDefault")}
            </button>
          </div>
          <p className="settings-panel__hint">{t("cacheLocationHint")}</p>
          <div className="settings-panel__limit-row">
            <label htmlFor="cache-limit">{t("cacheLimit")}</label>
            <div className="settings-panel__limit-input">
              <input
                aria-invalid={!limitIsValid}
                id="cache-limit"
                max={MAX_CACHE_GB}
                min={MIN_CACHE_GB}
                onChange={(event) => setLimitGb(Number(event.target.value))}
                type="number"
                value={limitGb}
              />
              <span>GB</span>
            </div>
            <button
              disabled={!settings || !limitIsValid || cacheBusy || limitGb === cacheLimitGb(settings.maxSizeBytes)}
              onClick={() => settings && updateCache.mutate({
                customParent: settings.customParent ?? null,
                maxSizeBytes: limitGb * GIB,
              })}
            >
              {t("apply")}
            </button>
          </div>
          <div className="settings-panel__cache-footer">
            <span>{t("cacheLimitRange")}</span>
            <button
              className={clearArmed ? "is-armed" : ""}
              disabled={!settings || cacheBusy}
              onClick={() => clearArmed ? clearCache.mutate() : setClearArmed(true)}
            >
              {clearCache.isPending ? <LoaderCircle className="is-spinning" size={12} /> : <Trash2 size={12} />}
              {clearArmed ? t("confirmClearCache") : t("clearCache")}
            </button>
          </div>
          {cacheError ? <p className="settings-panel__error">{String(cacheError)}</p> : null}
        </div>

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
          <span className="settings-panel__label">{t("metadataDisplay")}</span>
          <div className="settings-panel__options">
            {[
              {
                enabled: gridMetadataVisible,
                label: t("showGridMetadata"),
                toggle: () => setGridMetadataVisible(!gridMetadataVisible),
              },
              {
                enabled: loupeMetadataVisible,
                label: t("showLoupeMetadata"),
                toggle: () => setLoupeMetadataVisible(!loupeMetadataVisible),
              },
            ].map(({ enabled, label, toggle }) => (
              <button
                key={label}
                aria-pressed={enabled}
                className={`settings-panel__option ${enabled ? "is-active" : ""}`}
                onClick={toggle}
              >
                <span>{label}</span>
                {enabled ? <Check size={12} /> : null}
              </button>
            ))}
          </div>
        </div>

        <div className="settings-panel__section">
          <span className="settings-panel__label">{t("displaySharpening")}</span>
          <div className="settings-panel__options">
            {[
              { enabled: true, label: t("standard") },
              { enabled: false, label: t("disabled") },
            ].map(({ enabled, label }) => (
              <button
                key={String(enabled)}
                className={`settings-panel__option ${displaySharpening === enabled ? "is-active" : ""}`}
                onClick={() => setDisplaySharpening(enabled)}
              >
                <span>{label}</span>
                {displaySharpening === enabled ? <Check size={12} /> : null}
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
                {diagnostics.data.backend} · queue {diagnostics.data.queueWaitMs} ms
                {` · decode ${diagnostics.data.decodeMs} ms · publish ${diagnostics.data.tilePublishMs} ms`}
                {` · total ${diagnostics.data.totalMs} ms`}
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
