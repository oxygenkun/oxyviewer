import { useCallback, useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  FolderOpen,
  Images,
  Layers,
  Library,
  ListChecks,
  Loader2,
  Play,
  RefreshCw,
  ScanFace,
  SlidersHorizontal,
  Square,
  Trash2,
  UserPlus,
  Users,
  X,
} from "lucide-react";

import {
  cancelFaceAnalysis,
  clearFaceDecision,
  closeFaceWorkbench,
  createPerson,
  decideFace,
  deletePerson,
  getFaceCalibration,
  getFaceCapability,
  getFaceClusters,
  getFaceReviewPage,
  getPersonUndo,
  listPersons,
  mergePersons,
  notifyFaceAssetReveal,
  onFaceAnalysisProgress,
  onFaceLibraryUpdated,
  onFaceWorkbenchContext,
  removeFacesFromPerson,
  renamePerson,
  requestFaceWorkbenchContext,
  resolveFaceObservation,
  startFaceAnalysis,
  undoPersonOperation,
  updateFaceAnalyzerSettings,
} from "@/lib/api";
import type {
  FaceAnalysisProgress,
  FaceAnalysisRequest,
  FaceCluster,
  FaceMatchSensitivity,
  FaceReviewFilter,
  FaceReviewItem,
  FaceWorkbenchContext,
  Person,
  UndoableOperation,
} from "@/types";
import { RevealableFaceCrop, useFaceCrops } from "@/components/people/FaceCrop";
import { translate, type Locale, type MessageKey } from "@/lib/i18n";

/** Faces shown per cluster before the remainder is summarized as "+N". */
const CLUSTER_PREVIEW_LIMIT = 12;

const SENSITIVITIES: FaceMatchSensitivity[] = ["strict", "balanced", "loose"];

type WorkbenchTab = "clusters" | "persons" | "review";

const TABS: Array<{ id: WorkbenchTab; label: MessageKey; icon: typeof Layers }> = [
  { id: "clusters", label: "peopleClustersShort", icon: Layers },
  { id: "persons", label: "peoplePersons", icon: Users },
  { id: "review", label: "peopleReview", icon: ListChecks },
];

/**
 * Human-readable description of the pending person operation.
 *
 * The undo button must say what it will undo: "merge" and "detach" are both
 * destructive-looking, and an unlabelled undo invites the wrong click.
 */
function undoLabel(
  operation: UndoableOperation,
  t: (key: MessageKey) => string,
): string {
  if (operation.kind === "mergePersons") {
    return t("peopleUndoMerge")
      .replace("{name}", operation.otherPersonName ?? "")
      .replace("{count}", String(operation.faceCount));
  }
  if (operation.kind === "assignFaces") {
    return t("peopleUndoAssign").replace("{count}", String(operation.faceCount));
  }
  return t("peopleUndoDetach").replace("{count}", String(operation.faceCount));
}

/**
 * Members worth showing first: the representative, then the members the user is
 * expected to confirm, then the boundary members. Truncating by this order
 * means a 200-member cluster still shows a recognizable sample instead of 12
 * arbitrary faces.
 */
function orderedClusterFaces(cluster: FaceCluster, limit: number): string[] {
  const outliers = new Set(cluster.outlierObservationIds);
  const ordered = [
    cluster.representativeObservationId,
    ...cluster.observationIds.filter((id) => !outliers.has(id) && id !== cluster.representativeObservationId),
    ...cluster.observationIds.filter((id) => outliers.has(id)),
  ];
  const seen = new Set<string>();
  const unique: string[] = [];
  for (const id of ordered) {
    if (seen.has(id)) continue;
    seen.add(id);
    unique.push(id);
    if (unique.length === limit) break;
  }
  return unique;
}

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
  const [tab, setTab] = useState<WorkbenchTab>("clusters");
  // Faces the user has not answered yet, not just the ones the matcher had an
  // opinion about: on a library with no named people every face is `unknown`,
  // so a `pending` default would show an empty queue right after analysis.
  const [filter, setFilter] = useState<FaceReviewFilter>("unreviewed");
  const [progress, setProgress] = useState<FaceAnalysisProgress | undefined>();
  const [draftNames, setDraftNames] = useState<Record<string, string>>({});
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [mergeSource, setMergeSource] = useState("");
  const [revealNotice, setRevealNotice] = useState<{ kind: "ok" | "error"; text: string }>();
  const [parametersOpen, setParametersOpen] = useState(false);

  const locale = context?.locale ?? localeOverride ?? "zh-CN";
  const t = useMemo(
    () => tOverride ?? ((key: MessageKey) => translate(locale, key)),
    [locale, tOverride],
  );

  const visiblePaths = context?.visiblePaths ?? [];
  const browseScope = context?.browseScope;

  const capability = useQuery({ queryKey: ["face-capability"], queryFn: getFaceCapability });
  const persons = useQuery({ queryKey: ["face-persons"], queryFn: listPersons });
  const clusters = useQuery({ queryKey: ["face-clusters"], queryFn: getFaceClusters });
  const review = useQuery({
    queryKey: ["face-review", filter],
    queryFn: () => getFaceReviewPage(filter),
    enabled: tab === "review",
  });
  const undoState = useQuery({ queryKey: ["face-undo"], queryFn: getPersonUndo });
  const calibration = useQuery({ queryKey: ["face-calibration"], queryFn: getFaceCalibration });

  const invalidate = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ["face-capability"] });
    void queryClient.invalidateQueries({ queryKey: ["face-persons"] });
    void queryClient.invalidateQueries({ queryKey: ["face-clusters"] });
    void queryClient.invalidateQueries({ queryKey: ["face-review"] });
    void queryClient.invalidateQueries({ queryKey: ["face-undo"] });
    void queryClient.invalidateQueries({ queryKey: ["face-calibration"] });
    void queryClient.invalidateQueries({ queryKey: ["asset-face-reviews"] });
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
    void onFaceAnalysisProgress((next) => {
      setProgress(next);
      // A finished run changed observations, clusters, and candidates.
      if (next.stage === "complete" || next.stage === "failed") invalidate();
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
      stopProgress?.();
      stopLibrary?.();
    };
  }, [invalidate]);

  useEffect(() => {
    if (!revealNotice) return;
    const timer = window.setTimeout(() => setRevealNotice(undefined), 2600);
    return () => window.clearTimeout(timer);
  }, [revealNotice]);

  const start = useMutation({
    mutationFn: (request: FaceAnalysisRequest) => startFaceAnalysis(request),
    onSuccess: invalidate,
  });
  const stop = useMutation({
    mutationFn: (jobId: string) => cancelFaceAnalysis(jobId),
    onSuccess: invalidate,
  });
  const updateSettings = useMutation({
    mutationFn: updateFaceAnalyzerSettings,
    onSuccess: invalidate,
  });
  const decide = useMutation({
    mutationFn: ({ observationId, decision }: { observationId: string; decision: Parameters<typeof decideFace>[1] }) =>
      decideFace(observationId, decision),
    onSuccess: invalidate,
  });
  const undoDecision = useMutation({ mutationFn: clearFaceDecision, onSuccess: invalidate });
  const create = useMutation({
    mutationFn: ({ name, observationIds }: { name: string; observationIds: string[] }) =>
      createPerson(crypto.randomUUID(), name).then(async (person) => {
        for (const observationId of observationIds) {
          await decideFace(observationId, { decision: "confirmPerson", personId: person.personId });
        }
        return person;
      }),
    onSuccess: (_person, variables) => {
      setDraftNames((current) => {
        const next = { ...current };
        for (const observationId of variables.observationIds) delete next[observationId];
        return next;
      });
      invalidate();
    },
  });
  const removePerson = useMutation({ mutationFn: deletePerson, onSuccess: invalidate });
  const merge = useMutation({
    mutationFn: ({ source, target }: { source: string; target: string }) =>
      mergePersons(source, target),
    onSuccess: () => {
      setMergeSource("");
      invalidate();
    },
  });
  const detach = useMutation({
    mutationFn: ({ personId, observationIds }: { personId: string; observationIds: string[] }) =>
      removeFacesFromPerson(personId, observationIds),
    onSuccess: invalidate,
  });
  const undo = useMutation({ mutationFn: undoPersonOperation, onSuccess: invalidate });
  const rename = useMutation({
    mutationFn: ({ personId, name }: { personId: string; name: string }) => renamePerson(personId, name),
    onSuccess: () => {
      setRenaming(null);
      invalidate();
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

  // Load only the visible section, in small independent batches. Full decodes
  // can be slow, so the first completed faces should appear without waiting
  // for every member of a large cluster.
  const clusterFaceIds = useMemo(() => {
    const ids: string[] = [];
    for (const cluster of clusters.data ?? []) {
      ids.push(...orderedClusterFaces(cluster, CLUSTER_PREVIEW_LIMIT));
    }
    return ids;
  }, [clusters.data]);
  const clusterCrops = useFaceCrops(tab === "clusters" ? clusterFaceIds : []);
  const reviewFaceIds = useMemo(
    () => (review.data?.items ?? []).map((item) => item.observationId),
    [review.data],
  );
  const reviewCrops = useFaceCrops(tab === "review" ? reviewFaceIds : []);
  const personCoverIds = useMemo(
    () => (persons.data ?? []).flatMap((person) => person.coverObservationId ? [person.coverObservationId] : []),
    [persons.data],
  );
  const personCovers = useFaceCrops(tab === "persons" ? personCoverIds : []);

  const activeJob = progress?.jobId ?? capability.data?.progress?.jobId;
  const running = capability.data?.running || start.isPending;
  const available = capability.data?.available ?? true;
  const stats = capability.data?.stats;
  const settings = capability.data?.settings;
  const pendingCount = stats?.pendingReviews ?? 0;
  const unknownCount = stats?.unknownFaces ?? 0;
  const clusterCount = clusters.data?.length ?? 0;

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
    if (browseScope) {
      start.mutate({ rootPath: browseScope.rootPath, directory: browseScope.directory });
      return;
    }
    start.mutate({ force: false });
  };

  const revealFace = (observationId: string) => reveal.mutate(observationId);
  const revealTitle = t("faceWorkbenchRevealHint");
  const reviewItems = review.data?.items ?? [];
  const clusterList = clusters.data ?? [];
  const personList = persons.data ?? [];

  const tabCount: Record<WorkbenchTab, number> = {
    clusters: clusterCount,
    persons: personList.length,
    review: pendingCount,
  };

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
              disabled={!available || running}
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

            <div className="face-launch__scopes">
              <button
                disabled={!available || running || !browseScope}
                onClick={() => browseScope && start.mutate({
                  rootPath: browseScope.rootPath,
                  directory: browseScope.directory,
                })}
                title={t("peopleAnalyzeFolderHint")}
                type="button"
              >
                <FolderOpen size={14} /> {t("peopleAnalyzeFolder")}
              </button>
              <button
                disabled={!available || running || visiblePaths.length === 0}
                onClick={() => start.mutate({ paths: visiblePaths })}
                title={t("faceWorkbenchSelectionHint").replace("{count}", String(visiblePaths.length))}
                type="button"
              >
                <Images size={14} /> {t("peopleAnalyzeSelection")}
                {visiblePaths.length > 0 ? <em>{visiblePaths.length.toLocaleString()}</em> : null}
              </button>
              <button
                disabled={!available || running}
                onClick={() => start.mutate({ force: false })}
                type="button"
              >
                <Library size={14} /> {t("peopleAnalyzeLibrary")}
              </button>
            </div>

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
                  <label className="people-panel__field">
                    <span>{t("peopleMatchSensitivity")}</span>
                    <select
                      onChange={(event) =>
                        updateSettings.mutate({ ...settings, matchSensitivity: event.target.value as FaceMatchSensitivity })
                      }
                      value={settings.matchSensitivity}
                    >
                      {SENSITIVITIES.map((value) => (
                        <option key={value} value={value}>
                          {t(`peopleSensitivity_${value}` as MessageKey)}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label className="people-panel__field">
                    <span>{t("peopleDetectionConfidence")}</span>
                    <input
                      max={0.99}
                      min={0.3}
                      onChange={(event) => updateSettings.mutate({ ...settings, detectionConfidence: Number(event.target.value) })}
                      step={0.01}
                      type="range"
                      value={settings.detectionConfidence}
                    />
                    <output>{settings.detectionConfidence.toFixed(2)}</output>
                  </label>
                  <label className="people-panel__toggle">
                    <input
                      checked={settings.detectSmallFaces}
                      onChange={(event) =>
                        updateSettings.mutate({ ...settings, detectSmallFaces: event.target.checked })
                      }
                      type="checkbox"
                    />
                    <span>{t("peopleDetectSmallFaces")}</span>
                  </label>
                  <p className="people-panel__hint">{t("peopleDetectSmallFacesHint")}</p>
                  <label className="people-panel__field">
                    <span>{t("peopleClusterThreshold")}</span>
                    <input
                      max={0.9}
                      min={0.1}
                      onChange={(event) => updateSettings.mutate({ ...settings, clusterThreshold: Number(event.target.value) })}
                      step={0.01}
                      type="range"
                      value={settings.clusterThreshold}
                    />
                    <output>{settings.clusterThreshold.toFixed(2)}</output>
                  </label>
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

          {capability.data?.peopleStorePath ? (
            <p className="face-launch__store" title={capability.data.peopleStorePath}>
              {t("peopleStorePath").replace("{path}", capability.data.peopleStorePath)}
            </p>
          ) : null}
        </aside>

        <main className="face-workbench__classification">
          <nav aria-label={t("faceWorkbenchTitle")} className="face-workbench__tabs" role="tablist">
            {TABS.map(({ id, label, icon: Icon }) => (
              <button
                aria-selected={tab === id}
                className={tab === id ? "is-active" : ""}
                key={id}
                onClick={() => setTab(id)}
                role="tab"
                type="button"
              >
                <Icon size={15} />
                <span>{t(label)}</span>
                <em>{tabCount[id].toLocaleString()}</em>
              </button>
            ))}
          </nav>

          <div className="face-workbench__panel">
            {tab === "clusters" ? (
              <section className="face-workbench__section">
                <header className="face-workbench__section-head">
                  <h2>{t("peopleClusters").replace("{count}", String(clusterCount))}</h2>
                  <p>{t("faceWorkbenchClustersHint")}</p>
                </header>
                {clusterList.length ? (
                  <ul className="people-panel__clusters">
                    {clusterList.map((cluster: FaceCluster) => (
                      <li key={cluster.clusterId}>
                        <div className="people-panel__cluster-faces">
                          {orderedClusterFaces(cluster, CLUSTER_PREVIEW_LIMIT).map((observationId) => {
                            const outlier = cluster.outlierObservationIds.includes(observationId);
                            return (
                              <RevealableFaceCrop
                                crop={clusterCrops.byObservation.get(observationId)}
                                key={observationId}
                                label={outlier ? t("peopleClusterOutlierFace") : t("peopleClusterMemberFace")}
                                outlier={outlier}
                                onReveal={() => revealFace(observationId)}
                                revealTitle={revealTitle}
                                size={72}
                              />
                            );
                          })}
                          {cluster.memberCount > CLUSTER_PREVIEW_LIMIT ? (
                            <span className="people-panel__badge">
                              {t("peopleClusterMore").replace(
                                "{count}",
                                String(cluster.memberCount - CLUSTER_PREVIEW_LIMIT),
                              )}
                            </span>
                          ) : null}
                        </div>
                        <div className="people-panel__cluster-summary">
                          <strong>{t("peopleClusterMembers").replace("{count}", String(cluster.memberCount))}</strong>
                          <span>{t("peopleClusterCohesion").replace("{value}", cluster.cohesion.toFixed(2))}</span>
                          {cluster.outlierObservationIds.length > 0 ? (
                            <span className="people-panel__badge">
                              {t("peopleClusterOutliers").replace(
                                "{count}",
                                String(cluster.outlierObservationIds.length),
                              )}
                            </span>
                          ) : null}
                        </div>
                        <div className="people-panel__cluster-actions">
                          <input
                            onChange={(event) =>
                              setDraftNames((current) => ({ ...current, [cluster.clusterId]: event.target.value }))
                            }
                            placeholder={t("peopleNamePlaceholder")}
                            value={draftNames[cluster.clusterId] ?? ""}
                          />
                          <button
                            disabled={!draftNames[cluster.clusterId]?.trim() || create.isPending}
                            onClick={() =>
                              create.mutate({
                                name: draftNames[cluster.clusterId].trim(),
                                // Outliers stay unconfirmed: they are the
                                // members a reviewer should look at, not the
                                // members to accept.
                                observationIds: cluster.observationIds.filter(
                                  (id) => !cluster.outlierObservationIds.includes(id),
                                ),
                              })
                            }
                            type="button"
                          >
                            <UserPlus size={14} /> {t("peopleConfirmCluster")}
                          </button>
                        </div>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <p className="people-panel__empty">{t("peopleNoClusters")}</p>
                )}
              </section>
            ) : null}

            {tab === "persons" ? (
              <section className="face-workbench__section">
                <header className="face-workbench__section-head">
                  <h2>{t("peoplePersons")}</h2>
                  <p>{t("faceWorkbenchPersonsHint")}</p>
                </header>
                {undoState.data ? (
                  <div className="people-panel__undo">
                    <button disabled={undo.isPending} onClick={() => undo.mutate()} type="button">
                      ↶ {undoLabel(undoState.data, t)}
                    </button>
                  </div>
                ) : null}
                {personList.length ? (
                  <ul className="people-panel__persons">
                    {personList.map((person: Person) => (
                      <li key={person.personId}>
                        {person.coverObservationId ? (
                          <RevealableFaceCrop
                            crop={personCovers.byObservation.get(person.coverObservationId)}
                            label={person.displayName}
                            onReveal={() => revealFace(person.coverObservationId!)}
                            revealTitle={revealTitle}
                            size={42}
                          />
                        ) : null}
                        {renaming === person.personId ? (
                          <>
                            <input
                              autoFocus
                              onChange={(event) => setRenameValue(event.target.value)}
                              onKeyDown={(event) => {
                                if (event.key === "Enter" && renameValue.trim()) {
                                  rename.mutate({ personId: person.personId, name: renameValue });
                                }
                                if (event.key === "Escape") setRenaming(null);
                              }}
                              value={renameValue}
                            />
                            <button
                              aria-label={t("peopleRenameConfirm")}
                              disabled={!renameValue.trim() || rename.isPending}
                              onClick={() => rename.mutate({ personId: person.personId, name: renameValue })}
                              type="button"
                            >
                              <Check size={14} />
                            </button>
                          </>
                        ) : (
                          <>
                            <button
                              className="people-panel__person-name"
                              onClick={() => {
                                setRenaming(person.personId);
                                setRenameValue(person.displayName);
                              }}
                              type="button"
                            >
                              {person.displayName}
                            </button>
                            <span className="people-panel__badge">
                              {t("peoplePersonFaces").replace("{count}", String(person.faceCount))}
                            </span>
                            <button
                              aria-label={t("peopleDeletePerson").replace("{name}", person.displayName)}
                              onClick={() => removePerson.mutate(person.personId)}
                              type="button"
                            >
                              <Trash2 size={14} />
                            </button>
                          </>
                        )}
                      </li>
                    ))}
                  </ul>
                ) : (
                  <p className="people-panel__empty">{t("peopleNoPersons")}</p>
                )}
                {personList.length > 1 ? (
                  <label className="people-panel__field">
                    <span>{t("peopleMergeHint")}</span>
                    <select onChange={(event) => setMergeSource(event.target.value)} value={mergeSource}>
                      <option value="">{t("peopleMergeChoose")}</option>
                      {personList.map((person: Person) => (
                        <option key={person.personId} value={person.personId}>
                          {person.displayName}
                        </option>
                      ))}
                    </select>
                    <select
                      disabled={!mergeSource}
                      onChange={(event) => {
                        if (!event.target.value) return;
                        merge.mutate({ source: mergeSource, target: event.target.value });
                      }}
                      value=""
                    >
                      <option value="">{t("peopleMergeInto")}</option>
                      {personList
                        .filter((person: Person) => person.personId !== mergeSource)
                        .map((person: Person) => (
                          <option key={person.personId} value={person.personId}>
                            {person.displayName}
                          </option>
                        ))}
                    </select>
                  </label>
                ) : null}
              </section>
            ) : null}

            {tab === "review" ? (
              <section className="face-workbench__section">
                <header className="face-workbench__section-head">
                  <h2>{t("peopleReview")}</h2>
                  <p>{t("faceWorkbenchReviewHint")}</p>
                </header>
                <div className="people-panel__filters">
                  {(["unreviewed", "pending", "unknown", "confirmed", "rejected", "all"] as FaceReviewFilter[]).map((value) => (
                    <button
                      className={filter === value ? "is-active" : ""}
                      key={value}
                      onClick={() => setFilter(value)}
                      type="button"
                    >
                      {t(`peopleFilter_${value}` as MessageKey)}
                    </button>
                  ))}
                </div>
                {review.isLoading ? (
                  <p className="people-panel__empty">
                    <Loader2 className="is-spinning" size={14} /> {t("peopleReviewLoading")}
                  </p>
                ) : reviewItems.length ? (
                  <ul className="people-panel__review">
                    {reviewItems.map((item: FaceReviewItem) => (
                      <li key={item.observationId}>
                        <div className="people-panel__review-head">
                          <RevealableFaceCrop
                            crop={reviewCrops.byObservation.get(item.observationId)}
                            label={fileName(item.assetPath)}
                            onReveal={() => revealFace(item.observationId)}
                            revealTitle={revealTitle}
                            size={64}
                          />
                          <div className="people-panel__review-meta">
                            <span title={item.assetPath}>{fileName(item.assetPath)}</span>
                            <span className="people-panel__badge">
                              {t("peopleDetectionScore").replace("{value}", item.detectionScore.toFixed(2))}
                            </span>
                            {item.candidate ? (
                              <span className="people-panel__badge">
                                {t("peopleCandidate")
                                  .replace("{name}", item.candidate.personName)
                                  .replace("{value}", item.candidate.similarity.toFixed(2))}
                              </span>
                            ) : null}
                            {item.confirmedPersonName ? (
                              <span className="people-panel__badge is-confirmed">{item.confirmedPersonName}</span>
                            ) : null}
                            <span className="people-panel__state">{t(`peopleState_${item.state}` as MessageKey)}</span>
                          </div>
                        </div>
                        <div className="people-panel__review-actions">
                          {item.candidate ? (
                            <button
                              disabled={decide.isPending}
                              onClick={() =>
                                decide.mutate({
                                  observationId: item.observationId,
                                  decision: { decision: "confirmPerson", personId: item.candidate!.personId },
                                })
                              }
                              title={t("peopleConfirmMatch")}
                              type="button"
                            >
                              <Check size={14} />
                            </button>
                          ) : null}
                          {item.candidate ? (
                            <button
                              disabled={decide.isPending}
                              onClick={() =>
                                decide.mutate({
                                  observationId: item.observationId,
                                  decision: { decision: "rejectPerson", personId: item.candidate!.personId },
                                })
                              }
                              title={t("peopleRejectMatch")}
                              type="button"
                            >
                              <X size={14} />
                            </button>
                          ) : null}
                          <button
                            disabled={decide.isPending}
                            onClick={() =>
                              decide.mutate({ observationId: item.observationId, decision: { decision: "notFace" } })
                            }
                            title={t("peopleNotFace")}
                            type="button"
                          >
                            ⊘
                          </button>
                          {item.state === "confirmed" && item.confirmedPersonId ? (
                            <button
                              disabled={detach.isPending}
                              onClick={() =>
                                detach.mutate({
                                  personId: item.confirmedPersonId!,
                                  observationIds: [item.observationId],
                                })
                              }
                              title={t("peopleDetachFace").replace("{name}", item.confirmedPersonName ?? "")}
                              type="button"
                            >
                              ⤫ {t("peopleDetach")}
                            </button>
                          ) : null}
                          {item.state !== "unknown" && item.state !== "pending" ? (
                            <button
                              disabled={decide.isPending}
                              onClick={() => undoDecision.mutate(item.observationId)}
                              title={t("peopleUndo")}
                              type="button"
                            >
                              ↺
                            </button>
                          ) : null}
                        </div>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <p className="people-panel__empty">{t("peopleReviewEmpty")}</p>
                )}
              </section>
            ) : null}
          </div>
        </main>
      </div>
    </div>
  );
}

/** Re-exported for the window bootstrap without importing React Query twice. */
export type { FaceWorkbenchProps };
