import { Button, IconButton } from "../ui/Button";
import { Field } from "../ui/Field";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import {
  Check,
  Database,
  ExternalLink,
  FolderOpen,
  HardDrive,
  Info,
  Keyboard,
  Monitor,
  RotateCcw,
  SlidersHorizontal,
  Trash2,
  X,
} from "lucide-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import {
  chooseCacheParent,
  clearPreviewCache,
  getCacheSettings,
  getHeifCapabilities,
  updateCacheSettings,
} from "@/lib/api";
import { clearImageProjections } from "@/lib/projection/imageProjection";
import {
  GIB,
  MAX_CACHE_GB,
  MIN_CACHE_GB,
  cacheLimitGb,
  formatBytes,
  validCacheLimitGb,
} from "@/lib/cache/cacheSettings";
import type { Locale, MessageKey } from "@/lib/i18n";
import { LOUPE_ZOOM_SENSITIVITY, MAX_FULL_PREDECODE_COUNT, UI_FONT_SCALES } from "@/lib/browse/workspacePersistence";
import { useWorkspaceStore } from "@/store";
import { RawDecoderPanel } from "./RawDecoderPanel";
import { AboutSettings } from "./AboutSettings";
import { ExternalAppsSettings } from "./ExternalAppsSettings";
import { ShortcutSettings } from "./ShortcutSettings";
import type { AssetSummary } from "@/types";

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
  const previousFocus = useRef<HTMLElement | null>(null);
  const {
    displaySharpening,
    fullPredecodeCount,
    setFullPredecodeCount,
    gridMetadataVisible,
    locale,
    loupeMetadataVisible,
    loupeZoomSensitivity,
    peopleModuleVisible,
    setPeopleModuleVisible,
    uiFontScale,
    setDisplaySharpening,
    setGridMetadataVisible,
    setLocale,
    setLoupeMetadataVisible,
    setLoupeZoomSensitivity,
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
    { id: "about" as const, label: t("settingsAbout"), description: t("settingsAboutDescription"), icon: <Info size={16} /> },
  ];
  const activeTab = tabs.find((tab) => tab.id === settingsSection) ?? tabs[0];

  return (
    <DialogPrimitive.Root open onOpenChange={(open) => { if (!open) toggleSettings(); }}>
      <DialogPrimitive.Overlay className="settings-overlay">
      <DialogPrimitive.Content
        className="settings-panel"
        onOpenAutoFocus={() => {
          previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          if (previousFocus.current?.isConnected) previousFocus.current.focus({ preventScroll: true });
        }}
        onKeyDown={(event) => event.stopPropagation()}
      >
        <header className="settings-panel__header">
          <div>
            <DialogPrimitive.Title asChild><span>{t("settings")}</span></DialogPrimitive.Title>
            <DialogPrimitive.Description asChild><small>{t("settingsSubtitle")}</small></DialogPrimitive.Description>
          </div>
          <IconButton label={t("closeSettings")} onClick={toggleSettings}><X size={18} /></IconButton>
        </header>
        <div className="settings-panel__body">
          <nav aria-label={t("settings")} className="settings-panel__tabs" role="tablist">
            {tabs.map((tab) => (
              <button
                aria-controls="settings-tabpanel"
                aria-selected={settingsSection === tab.id}
                className={[
                  settingsSection === tab.id ? "is-active" : "",
                  tab.id === "about" ? "is-pinned" : "",
                ].filter(Boolean).join(" ")}
                id={`settings-tab-${tab.id}`}
                key={tab.id}
                onClick={() => setSettingsSection(tab.id)}
                role="tab"
                tabIndex={settingsSection === tab.id ? 0 : -1}
                onKeyDown={(event) => {
                  const step = event.key === "ArrowDown" || event.key === "ArrowRight" ? 1
                    : event.key === "ArrowUp" || event.key === "ArrowLeft" ? -1 : 0;
                  if (!step && event.key !== "Home" && event.key !== "End") return;
                  event.preventDefault();
                  const index = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1
                    : (tabs.findIndex((item) => item.id === tab.id) + step + tabs.length) % tabs.length;
                  setSettingsSection(tabs[index].id);
                  document.getElementById(`settings-tab-${tabs[index].id}`)?.focus();
                }}
              >
                {tab.icon}
                <span>{tab.label}</span>
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
                <span className="settings-panel__label">{t("modules")}</span>
                <div className="settings-panel__options">
                  <Button aria-pressed={peopleModuleVisible} className="settings-panel__option" onClick={() => setPeopleModuleVisible(!peopleModuleVisible)}>
                    <span>{t("showPeopleModule")}</span>{peopleModuleVisible ? <Check size={14} /> : null}
                  </Button>
                </div>
                <p className="settings-panel__hint">{t("showPeopleModuleHint")}</p>
              </section>
              <section className="settings-panel__section">
                <span className="settings-panel__label">{t("language")}</span>
                <div className="settings-panel__options">
                  {languages.map(({ value, label }) => (
                    <Button key={value} aria-pressed={locale === value} className="settings-panel__option" onClick={() => setLocale(value)}>
                      <span>{label}</span>{locale === value ? <Check size={14} /> : null}
                    </Button>
                  ))}
                </div>
              </section>
              <section className="settings-panel__section">
                <span className="settings-panel__label">{t("appearance")}</span>
                <div className="settings-panel__field">
                  <span>{t("uiFontSize")}</span>
                  <div className="settings-panel__font-size-options">
                    {UI_FONT_SCALES.map((scale) => (
                      <Button size="small" variant="ghost" key={scale} aria-label={`${t("uiFontSize")} ${Math.round(scale * 100)}%`} aria-pressed={uiFontScale === scale} onClick={() => setUiFontScale(scale)}>
                        {Math.round(scale * 100)}%
                      </Button>
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
                <Field label={t("loupeZoomSensitivity")} id="loupe-zoom-sensitivity" type="range"
                  min={LOUPE_ZOOM_SENSITIVITY.min} max={LOUPE_ZOOM_SENSITIVITY.max} step={LOUPE_ZOOM_SENSITIVITY.step}
                  value={loupeZoomSensitivity} aria-valuetext={`${loupeZoomSensitivity}%`} suffix={`${loupeZoomSensitivity}%`}
                  hint={t("loupeZoomSensitivityHint")}
                  onChange={(event) => setLoupeZoomSensitivity(event.target.valueAsNumber)} />
                {navigator.platform.startsWith("Mac") ? (
                  <p className="settings-panel__hint">{t("loupeThreeFingerDragHint")}</p>
                ) : null}
              </section>
              <section className="settings-panel__section">
                <span className="settings-panel__label">{t("metadataDisplay")}</span>
                <div className="settings-panel__options">
                  {[
                    { enabled: gridMetadataVisible, label: t("showGridMetadata"), toggle: () => setGridMetadataVisible(!gridMetadataVisible) },
                    { enabled: loupeMetadataVisible, label: t("showLoupeMetadata"), toggle: () => setLoupeMetadataVisible(!loupeMetadataVisible) },
                  ].map(({ enabled, label, toggle }) => (
                    <Button key={label} aria-pressed={enabled} className="settings-panel__option" onClick={toggle}>
                      <span>{label}</span>{enabled ? <Check size={14} /> : null}
                    </Button>
                  ))}
                </div>
              </section>
              <section className="settings-panel__section">
                <span className="settings-panel__label">{t("displaySharpening")}</span>
                <div className="settings-panel__options">
                  {[{ enabled: true, label: t("standard") }, { enabled: false, label: t("disabled") }].map(({ enabled, label }) => (
                    <Button key={String(enabled)} aria-pressed={displaySharpening === enabled} className="settings-panel__option" onClick={() => setDisplaySharpening(enabled)}>
                      <span>{label}</span>{displaySharpening === enabled ? <Check size={14} /> : null}
                    </Button>
                  ))}
                </div>
              </section>
            </> : null}

            {settingsSection === "media" ? <>
              <section className="settings-panel__section">
                <Field label={t("fullPredecodeCount")} id="full-predecode-count" type="number"
                  min={0} max={MAX_FULL_PREDECODE_COUNT} step={1} value={fullPredecodeCount}
                  onChange={(event) => setFullPredecodeCount(Number(event.target.value))} />
                <p className="settings-panel__hint">{t("fullPredecodeHint")}</p>
              </section>
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
                  <Button disabled={!settings || cacheBusy} onClick={chooseLocation}><FolderOpen size={14} /> {t("chooseLocation")}</Button>
                  <Button disabled={!settings?.isCustomLocation || cacheBusy} onClick={() => settings && updateCache.mutate({ customParent: null, maxSizeBytes: settings.maxSizeBytes })}><RotateCcw size={14} /> {t("restoreDefault")}</Button>
                </div>
                <p className="settings-panel__hint">{t("cacheLocationHint")}</p>
                <div className="settings-panel__limit-row">
                  <Field label={t("cacheLimit")} suffix="GB" aria-invalid={!limitIsValid} id="cache-limit" max={MAX_CACHE_GB} min={MIN_CACHE_GB} onChange={event => setLimitGb(Number(event.target.value))} type="number" value={limitGb} />
                  <Button disabled={!settings || !limitIsValid || cacheBusy || limitGb === cacheLimitGb(settings.maxSizeBytes)} onClick={() => settings && updateCache.mutate({ customParent: settings.customParent ?? null, maxSizeBytes: limitGb * GIB })}>{t("apply")}</Button>
                </div>
                <div className="settings-panel__cache-footer">
                  <span>{t("cacheLimitRange")}</span>
                  <Button variant="danger" loading={clearCache.isPending} className={clearArmed ? "is-armed" : ""} disabled={!settings || cacheBusy} onClick={() => clearArmed ? clearCache.mutate() : setClearArmed(true)}>
                    {!clearCache.isPending ? <Trash2 size={14} /> : null}{clearArmed ? t("confirmClearCache") : t("clearCache")}
                  </Button>
                </div>
                {cacheError ? <p className="settings-panel__error" role="alert">{String(cacheError)}</p> : null}
              </section>
              <section className="settings-panel__section"><RawDecoderPanel t={t} asset={activeAsset?.kind === "raw" ? activeAsset : undefined} /></section>
              <section className="settings-panel__section">
                <div className="settings-panel__diagnostics"><small>{t("heifDiagnostics")}</small>{capabilities.data?.map((capability) => <span key={capability.backend}>{capability.backend}: {capability.available ? capability.acceleration : t("unavailable")}</span>)}</div>
              </section>
            </> : null}

            {settingsSection === "externalApps" ? <ExternalAppsSettings t={t} focus /> : null}

            {settingsSection === "shortcuts" ? <ShortcutSettings t={t} /> : null}

            {settingsSection === "about" ? <AboutSettings t={t} /> : null}
          </div>
        </div>
      </DialogPrimitive.Content>
      </DialogPrimitive.Overlay>
    </DialogPrimitive.Root>
  );
}
