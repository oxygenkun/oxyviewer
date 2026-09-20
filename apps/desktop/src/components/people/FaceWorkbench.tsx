import { FacePhotoReview } from "./FacePhotoReview";
import { FaceModelPanel } from "./FaceModelPanel";
import { PersonManager } from "./PersonManager";
import { SettingsRenderer } from "@/components/analyzers/SettingsRenderer";
import { faceSettingsDescriptor } from "./faceDescriptors";
import { FaceSyncPanel } from "./FaceSyncPanel";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Loader2,
  Play,
  RefreshCw,
  ScanFace,
  SlidersHorizontal,
  Square,
  X,
} from "lucide-react";

import {
  cancelFaceAnalysis,
  closeFaceWorkbench,
  getFaceCalibration,
  getFaceCapability,
  getFaceClusters,
  listPersons,
  installFaceModel,
  notifyFaceAssetReveal,
  onFaceAnalysisProgress,
  onFaceLibraryUpdated,
  onFaceModelDownloadProgress,
  onFaceWorkbenchContext,
  requestFaceWorkbenchContext,
  resolveFaceObservation,
  startFaceAnalysis,
  updateFaceAnalyzerSettings,
} from "@/lib/api";
import type {
  FaceAnalysisProgress,
  FaceAnalysisRequest,
  FaceModelDownloadProgress,
  FaceWorkbenchContext,
} from "@/types";
import { translate, type Locale, type MessageKey } from "@/lib/i18n";
import { invalidateFaceQueries } from "./faceQueryInvalidation";

function fileName(path: string): string {
  return path.split(/[\\/]/).at(-1) ?? path;
}

interface FaceWorkbenchProps {
  /** Test override; production derives `t` from the published locale. */
  t?: (key: MessageKey) => string;
  locale?: Locale;
}

/**
 * The face workbench: a separate, large window for launching analysis and
 * labeling what it found.
 *
 * It is deliberately not a settings tab. Analysis and labeling are the primary
 * people workflow, so the entry point sits next to Settings in the sidebar and
 * this window owns the space a review pass needs: a launch rail on the left and
 * the classification surface on the right.
 *
 * Clicking any face crop asks the main window to reveal the photo it came from
 * in its loupe, which is why the two windows publish state to each other rather
 * than sharing a store.
 */
export function FaceWorkbench({ t: tOverride, locale: localeOverride }: FaceWorkbenchProps = {}) {
  const queryClient = useQueryClient();
  const [context, setContext] = useState<FaceWorkbenchContext>();
  const [personsOpen, setPersonsOpen] = useState(false);
  const [progress, setProgress] = useState<FaceAnalysisProgress | undefined>();
  const [modelProgress, setModelProgress] = useState<FaceModelDownloadProgress | undefined>();
  const [revealNotice, setRevealNotice] = useState<{ kind: "ok" | "error"; text: string }>();
  const [parametersOpen, setParametersOpen] = useState(false);

  const locale = context?.locale ?? localeOverride ?? "zh-CN";
  const t = useMemo(
    () => tOverride ?? ((key: MessageKey) => translate(locale, key)),
    [locale, tOverride],
  );

  const browseScope = context?.browseScope;

  const capability = useQuery({ queryKey: ["face-capability"], queryFn: getFaceCapability });
  const persons = useQuery({ queryKey: ["face-persons"], queryFn: listPersons });
  const clusters = useQuery({ queryKey: ["face-clusters"], queryFn: getFaceClusters });
  const calibration = useQuery({ queryKey: ["face-calibration"], queryFn: getFaceCalibration });

  const invalidate = useCallback(() => {
    invalidateFaceQueries(queryClient, "people");
  }, [queryClient]);

  const invalidateAnalysis = useCallback(() => {
    invalidateFaceQueries(queryClient, "analysis");
  }, [queryClient]);

  // Detection commits each asset independently, so the review surface can
  // expose those observations while the rest of the folder is still running.
  // Keep this refresh deliberately narrow: clusters and candidates are only
  // published by the later derived pass, and refetching every workbench query
  // for each progress tick would make the controls jump under the user.
  const invalidateIncrementalResults = useCallback(() => {
    invalidateFaceQueries(queryClient, "incremental");
  }, [queryClient]);

  // The main window owns the browse scope; this window asks for it once and
  // then follows every change it publishes.
  useEffect(() => {
    let disposed = false;
    let stopContext: (() => void) | undefined;
    void onFaceWorkbenchContext((next) => setContext(next)).then((unlisten) => {
      if (disposed) unlisten();
      else stopContext = unlisten;
    });
    void requestFaceWorkbenchContext();
    return () => {
      disposed = true;
      stopContext?.();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let stopProgress: (() => void) | undefined;
    let stopLibrary: (() => void) | undefined;
    let activeJob = "";
    let lastProcessed = 0;
    let refreshQueued = false;
    let refreshCooldown: number | undefined;
    const refreshIncrementally = () => {
      invalidateIncrementalResults();
      refreshCooldown = window.setTimeout(() => {
        refreshCooldown = undefined;
        if (refreshQueued) {
          refreshQueued = false;
          refreshIncrementally();
        }
      }, 400);
    };
    const requestIncrementalRefresh = () => {
      if (refreshCooldown === undefined) refreshIncrementally();
      else refreshQueued = true;
    };
    void onFaceAnalysisProgress((next) => {
      setProgress(next);
      if (next.jobId !== activeJob) {
        activeJob = next.jobId;
        lastProcessed = 0;
      }
      if (next.stage === "detecting" && next.processedAssets > lastProcessed) {
        lastProcessed = next.processedAssets;
        requestIncrementalRefresh();
      }
    }).then((unlisten) => {
      if (disposed) unlisten();
      else stopProgress = unlisten;
    });
    void onFaceLibraryUpdated(() => invalidate()).then((unlisten) => {
      if (disposed) unlisten();
      else stopLibrary = unlisten;
    });
    return () => {
      disposed = true;
      if (refreshCooldown !== undefined) window.clearTimeout(refreshCooldown);
      stopProgress?.();
      stopLibrary?.();
    };
  }, [invalidate, invalidateIncrementalResults]);

  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void onFaceModelDownloadProgress((next) => setModelProgress(next)).then((unlisten) => {
      if (disposed) unlisten();
      else stop = unlisten;
    });
    return () => {
      disposed = true;
      stop?.();
    };
  }, []);

  useEffect(() => {
    if (!revealNotice) return;
    const timer = window.setTimeout(() => setRevealNotice(undefined), 2600);
    return () => window.clearTimeout(timer);
  }, [revealNotice]);

  const start = useMutation({
    mutationFn: (request: FaceAnalysisRequest) => startFaceAnalysis(request),
    onSuccess: invalidateAnalysis,
  });
  const stop = useMutation({
    mutationFn: (jobId: string) => cancelFaceAnalysis(jobId),
    onSuccess: invalidateAnalysis,
  });
  const updateSettings = useMutation({
    mutationFn: updateFaceAnalyzerSettings,
    onSuccess: invalidateAnalysis,
  });
  const installModel = useMutation({
    mutationFn: installFaceModel,
    onSuccess: (next) => {
      queryClient.setQueryData(["face-capability"], next);
      invalidateAnalysis();
    },
  });
  const reveal = useMutation({
    mutationFn: (observationId: string) => resolveFaceObservation(observationId),
    onSuccess: (location) => {
      if (!location) {
        setRevealNotice({ kind: "error", text: t("faceWorkbenchRevealMissing") });
        return;
      }
      void notifyFaceAssetReveal(location).then(() => {
        setRevealNotice({
          kind: "ok",
          text: t("faceWorkbenchRevealed").replace("{name}", fileName(location.assetPath)),
        });
      });
    },
  });

  const activeJob = progress?.jobId ?? capability.data?.progress?.jobId;
  const running = capability.data?.running || start.isPending;
  const available = capability.data?.available ?? true;
  const stats = capability.data?.stats;
  const settings = capability.data?.settings;
  const pendingCount = stats?.pendingReviews ?? 0;
  const unknownCount = stats?.unknownFaces ?? 0;
  const processed = progress?.processedAssets ?? capability.data?.progress?.processedAssets ?? 0;
  const analyzedTotal = progress?.totalAssets ?? capability.data?.progress?.totalAssets ?? 0;
  const progressPercent = analyzedTotal > 0
    ? Math.min(100, Math.round((processed / analyzedTotal) * 100))
    : 0;

  const heading = useMemo(() => {
    if (!available) return capability.data?.unavailableReason ?? t("peopleUnavailable");
    if (running) {
      return analyzedTotal > 0
        ? t("peopleAnalyzing").replace("{done}", String(processed)).replace("{total}", String(analyzedTotal))
        : t("peopleAnalyzingIndeterminate");
    }
    return t("peopleIdleHint");
  }, [analyzedTotal, available, capability.data, processed, running, t]);

  // What the last run actually produced. A run that decoded nothing, or that
  // failed at the clustering step, otherwise looks identical to a successful
  // one: the analyser counts a failed file as processed, so "已处理 990" alone
  // would claim the whole batch worked while every list stays empty.
  const runSummary = useMemo(() => {
    const last = progress ?? capability.data?.progress;
    if (!last) return undefined;
    if (last.stage === "failed") {
      return {
        kind: "error" as const,
        text: t("faceWorkbenchRunFailed").replace("{message}", last.message ?? ""),
      };
    }
    if (last.stage !== "complete") return undefined;
    const lines = [
      t("faceWorkbenchRunResult")
        .replace("{done}", last.processedAssets.toLocaleString())
        .replace("{faces}", last.facesDetected.toLocaleString()),
    ];
    if (last.failedAssets > 0) {
      lines.push(t("faceWorkbenchRunFailures").replace("{count}", last.failedAssets.toLocaleString()));
    }
    if (last.facesDetected === 0) {
      lines.push(t("faceWorkbenchRunNoFaces"));
    }
    return {
      kind: last.facesDetected === 0 || last.failedAssets > 0 ? ("warn" as const) : ("ok" as const),
      text: lines.join(" "),
    };
  }, [capability.data, progress, t]);

  const startScoped = () => {
    if (!browseScope) return;
    start.mutate({ rootPath: browseScope.rootPath, directory: browseScope.directory });
  };

  const revealFace = (observationId: string) => reveal.mutate(observationId);
  const revealTitle = t("faceWorkbenchRevealHint");
  const personList = persons.data ?? [];

  return (
    <div className="face-workbench">
      <header className="face-workbench__header">
        <div className="face-workbench__identity">
          <span className="face-workbench__mark"><ScanFace size={20} /></span>
          <div>
            <strong>{t("faceWorkbenchTitle")}</strong>
            <small>{heading}</small>
          </div>
        </div>
        <div className="face-workbench__header-actions">
          <span
            aria-live="polite"
            className={`face-workbench__notice${revealNotice ? ` is-${revealNotice.kind}` : ""}`}
          >
            {revealNotice?.text ?? t("faceWorkbenchSyncHint")}
          </span>
          <button onClick={invalidate} title={t("faceWorkbenchRefresh")} type="button">
            <RefreshCw size={14} /> {t("faceWorkbenchRefresh")}
          </button>
          <button onClick={() => void closeFaceWorkbench()} title={t("closeSettings")} type="button">
            <X size={14} /> {t("faceWorkbenchClose")}
          </button>
        </div>
      </header>

      <div className="face-workbench__body">
        <aside className="face-workbench__launch">
          <section className="face-launch">
            <span className="face-launch__eyebrow">{t("faceWorkbenchLaunch")}</span>
            <h2>{t("faceWorkbenchLaunchTitle")}</h2>
            <p className="face-launch__scope" title={browseScope?.directory}>
              {browseScope
                ? t("faceWorkbenchScopeFolder").replace("{name}", fileName(browseScope.directory))
                : t("faceWorkbenchScopeLibrary")}
            </p>

            <button
              className="face-launch__primary"
              disabled={!available || running || !browseScope}
              onClick={startScoped}
              type="button"
            >
              {running ? <Loader2 className="is-spinning" size={17} /> : <Play size={17} />}
              {running ? t("peopleAnalyzingIndeterminate") : t("peopleAnalyzeStart")}
            </button>

            {running && activeJob ? (
              <button
                className="face-launch__cancel"
                onClick={() => stop.mutate(activeJob)}
                type="button"
              >
                <Square size={14} /> {t("peopleCancel")}
              </button>
            ) : null}

            <div className="face-launch__progress">
              <div
                aria-hidden="true"
                className={`face-launch__meter${running && analyzedTotal === 0 ? " is-indeterminate" : ""}`}
              >
                <span style={{ width: `${progressPercent}%` }} />
              </div>
              <span>
                {analyzedTotal > 0
                  ? t("faceWorkbenchProgress")
                    .replace("{done}", processed.toLocaleString())
                    .replace("{total}", analyzedTotal.toLocaleString())
                  : t("faceWorkbenchProgressIdle")}
              </span>
            </div>

            {runSummary ? (
              <p className={`face-launch__summary is-${runSummary.kind}`} role="status">
                {runSummary.text}
              </p>
            ) : null}

            {stats ? (
              <dl className="face-launch__stats">
                <div><dt>{t("peopleStatAnalyzed")}</dt><dd>{stats.analyzedAssets.toLocaleString()}</dd></div>
                <div><dt>{t("peopleStatFacesLabel")}</dt><dd>{stats.facesDetected.toLocaleString()}</dd></div>
                <div><dt>{t("peopleStatPersonsLabel")}</dt><dd>{stats.persons.toLocaleString()}</dd></div>
                <div><dt>{t("peopleStatPendingLabel")}</dt><dd>{pendingCount.toLocaleString()}</dd></div>
                <div><dt>{t("peopleStatUnknownLabel")}</dt><dd>{unknownCount.toLocaleString()}</dd></div>
                <div><dt>{t("peopleStatClusters")}</dt><dd>{stats.clusters.toLocaleString()}</dd></div>
              </dl>
            ) : null}
          </section>

          <FaceModelPanel
            available={available}
            error={installModel.error}
            installing={installModel.isPending}
            installingId={installModel.variables}
            models={capability.data?.models ?? []}
            onInstall={(modelId) => installModel.mutate(modelId)}
            progress={modelProgress}
            running={running}
            t={t}
          />

          {settings ? (
            <section className="face-launch__block">
              <button
                aria-expanded={parametersOpen}
                className="face-launch__disclosure"
                onClick={() => setParametersOpen((open) => !open)}
                type="button"
              >
                <SlidersHorizontal size={14} />
                {t("peopleParameters")}
              </button>
              {parametersOpen ? (
                <div className="face-launch__parameters">
                  <SettingsRenderer descriptor={faceSettingsDescriptor(t)} values={{ ...settings }} disabled={updateSettings.isPending}
                    valueLabel={(_key, value) => ({ strict: t("peopleSensitivity_strict"), balanced: t("peopleSensitivity_balanced"), loose: t("peopleSensitivity_loose"), custom: t("faceMatchThreshold") }[value] ?? value)}
                    onChange={(key, value) => {
                      if (!faceSettingsDescriptor(t).fields.some((field) => field.key === key)) return;
                      updateSettings.mutate({ ...settings, [key]: value });
                    }} />
                  <p className="people-panel__hint">{t("peopleDetectSmallFacesHint")}</p>
                  {calibration.data ? (
                    <div className="people-panel__calibration">
                      <span className="settings-panel__label">{t("peopleCalibration")}</span>
                      <p className="people-panel__hint">
                        {t("peopleCalibrationSamples")
                          .replace("{accepted}", String(calibration.data.acceptedScores.length))
                          .replace("{rejected}", String(calibration.data.rejectedScores.length))}
                      </p>
                      {calibration.data.recommendedThreshold === undefined ? (
                        <p className="people-panel__hint">{t("peopleCalibrationInsufficient")}</p>
                      ) : (
                        <div className="people-panel__calibration-apply">
                          <span>
                            {t("peopleCalibrationRecommended").replace(
                              "{value}",
                              calibration.data.recommendedThreshold.toFixed(3),
                            )}
                          </span>
                          <button
                            disabled={updateSettings.isPending}
                            onClick={() =>
                              updateSettings.mutate({
                                ...settings,
                                matchSensitivity: "custom",
                                matchThreshold: calibration.data!.recommendedThreshold!,
                              })
                            }
                            type="button"
                          >
                            {t("peopleCalibrationApply")}
                          </button>
                        </div>
                      )}
                      {calibration.data.recommendedThreshold !== undefined && !calibration.data.separable ? (
                        <p className="people-panel__hint">{t("peopleCalibrationOverlap")}</p>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              ) : null}
            </section>
          ) : null}

          <FaceSyncPanel t={t} />
          {capability.data?.peopleStorePath ? (
            <p className="face-launch__store" title={capability.data.peopleStorePath}>
              {t("peopleStorePath").replace("{path}", capability.data.peopleStorePath)}
            </p>
          ) : null}
        </aside>

        <main className="face-workbench__classification">
          <div className="face-workbench__panel">
            <PersonManager
              invalidate={invalidate}
              onReveal={revealFace}
              open={personsOpen}
              persons={personList}
              revealTitle={revealTitle}
              setOpen={setPersonsOpen}
              t={t}
            />

            <FacePhotoReview
              persons={personList}
              clusters={clusters.data ?? []}
              directory={browseScope?.directory}
              rootPath={browseScope?.rootPath}
              t={t}
              onReveal={revealFace}
              invalidate={invalidate}
            />
          </div>
        </main>
      </div>
    </div>
  );
}

/** Re-exported for the window bootstrap without importing React Query twice. */
export type { FaceWorkbenchProps };
