import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { Aperture, CircleAlert, FolderPlus, RectangleHorizontal, RectangleVertical } from "lucide-react";
import { useCallback, useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { AssetBrowser } from "@/components/browsing/AssetBrowser";
import { BackgroundPreviewPreloader } from "@/components/loupe/BackgroundPreviewPreloader";
import { ImportOverlay } from "@/components/overlay/ImportOverlay";
import { Inspector } from "@/components/inspector/Inspector";
import { PerfHarness } from "@/components/common/PerfHarness";
import { SettingsPanel } from "@/components/settings/SettingsPanel";
import { Sidebar } from "@/components/browsing/Sidebar";
import { ResizeHandle } from "@/components/browsing/ResizeHandle";
import { Toolbar } from "@/components/browsing/Toolbar";
import {
  addLibraryRoot,
  chooseFolder,
  copyText,
  isTauri,
  listAssets,
  listLibraryRoots,
  openFolder,
  openInFileManager,
  openAssetWithApplication,
  openAssetWithSystemDialog,
  onDirectoryTreeUpdated,
  onDirectoryBrowseProgress,
  onLibraryDirectoryIndexUpdated,
  onLibraryIndexUpdated,
  onImageProjectionUpdated,
  onMetadataProjectionUpdated,
  refreshDirectory,
  requestMetadata,
  removeLibraryRoot,
  reorderLibraryRoots,
  setActiveDirectory,
  deletePaths,
} from "@/lib/api";
import { filterAndSortAssets } from "@/lib/assets/assetFiltering";
import { recordBrowseTiming } from "@/lib/diagnostics/browseDiagnostics";
import { firstBrowseCursor, nextBrowseCursor } from "@/lib/browse/browsePagination";
import { useBackgroundAssetPagination } from "@/lib/hooks/useBackgroundAssetPagination";
import { insertRestoredFolder, restoreFoldersProgressively, type FolderRestoreState } from "@/lib/browse/folderRestoration";
import {
  applyEntryFailure,
  applyEntryOpened,
  applyEntryRetried,
  attachImportRoot,
  applyIndexCompleted,
  beginFolderImport,
  dismissFolderImport,
  failFolderImport,
  failedImportEntries,
  folderImportSummary,
  IDLE_FOLDER_IMPORT,
  type FolderImportState,
} from "@/lib/browse/folderImport";
import { folderImportNotice } from "@/lib/browse/folderImportNotice";
import { useFolderDrop } from "@/lib/hooks/useFolderDrop";
import { activeAssetOrdinal, focusRestoreAction, replacementAssetIdAfterRemoval } from "@/lib/assets/assetViewPosition";
import { setBrowserImageResourceScope } from "@/lib/cache/browserImageCache";
import { acceptDirectoryTreeSnapshot } from "@/lib/projection/directoryTreeProjection";
import { acceptImageProjection, invalidateImageDirectory } from "@/lib/projection/imageProjection";
import {
  queueMetadataProjection,
  acceptMetadataProjections,
  invalidateMetadataDirectory,
  projectAssetMetadata,
  useMetadataProjectionStore,
} from "@/lib/projection/metadataProjection";
import { isSameOrDescendantPath, parentFolderPath, relativeFolderPath } from "@/lib/browse/folderPaths";
import { translate } from "@/lib/i18n";
import { LAYOUT_SIZE_LIMITS, maxInspectorWidth } from "@/lib/ui/layoutSizing";
import {
  mergeVisibleFolderOrder,
  sortFolderSessions,
  type FolderSort,
} from "@/lib/browse/folderOrdering";
import {
  completeFolderOnboarding,
  hasSeenFolderOnboarding,
  loadWorkspace,
  recoverMissingCurrentDirectory,
  saveWorkspace,
} from "@/lib/browse/workspacePersistence";
import { useWorkspaceStore } from "./store";
import type { AssetQuery, DirectoryBrowseProgress, DirectoryTreeSnapshot, FolderSession, MetadataProjection, PerfScenario } from "./types";

const NO_METADATA_RECORDS: Record<string, MetadataProjection> = {};

/** One status-bar message; the app's single place for transient feedback. */
interface StatusNotice {
  kind: "error" | "status";
  message: string;
  detail?: string;
  /** Offers the retry action for every failed import row. */
  retry?: boolean;
}

/** How long a clean import result stays in the status bar. */
const IMPORT_NOTICE_MS = 6000;

export function App({ perfScenario }: { perfScenario?: PerfScenario }) {
  const [workspace, setWorkspace] = useState(loadWorkspace);
  const [browseProgress, setBrowseProgress] = useState<DirectoryBrowseProgress>();
  const [folderRestoreStates, setFolderRestoreStates] = useState<FolderRestoreState[]>([]);
  const [folderImport, setFolderImport] = useState<FolderImportState>(IDLE_FOLDER_IMPORT);
  const [showOnboarding, setShowOnboarding] = useState(() => !hasSeenFolderOnboarding());
  const [error, setError] = useState<string>();
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [noticeExpanded, setNoticeExpanded] = useState(false);
  const queryClient = useQueryClient();
  const {
    view, thumbnailOrientation, activeId, selectedIds, inspectorOpen, leftPanelOpen, settingsOpen, locale,
    search, tagIds, tagMatch, clearSearch, kind, minimumRating, colorLabels, pickLabels, sort, direction, clearSelection, select, setThumbnailOrientation, toggleSettings,
    leftPanelWidth, inspectorWidth, setLeftPanelWidth, setInspectorWidth, uiFontScale,
  } = useWorkspaceStore();
  const appShellRef = useRef<HTMLDivElement>(null);
  const [appShellWidth, setAppShellWidth] = useState(0);
  const activeDirectoryNoticeRef = useRef<string | undefined>(undefined);
  const filteredFocusRef = useRef<string | undefined>(undefined);
  const t = useCallback((key: Parameters<typeof translate>[1]) => translate(locale, key), [locale]);

  useLayoutEffect(() => {
    document.documentElement.style.fontSize = `${uiFontScale * 100}%`;
    return () => {
      document.documentElement.style.removeProperty("font-size");
    };
  }, [uiFontScale]);

  useLayoutEffect(() => {
    const shell = appShellRef.current;
    if (!shell) return;
    const updateWidth = () => setAppShellWidth(shell.clientWidth);
    updateWidth();
    const observer = new ResizeObserver(updateWidth);
    observer.observe(shell);
    return () => observer.disconnect();
  }, []);

  const inspectorMax = maxInspectorWidth(appShellWidth, leftPanelOpen ? leftPanelWidth : 0);
  const displayedInspectorWidth = Math.min(inspectorWidth, inspectorMax);

  const foldersQuery = useQuery({
    queryKey: ["open-folders"],
    queryFn: async () => {
      const started = performance.now();
      const roots = await listLibraryRoots();
      recordBrowseTiming("workspace-roots", { elapsedMs: performance.now() - started, count: roots.length });
      await restoreFoldersProgressively(roots, workspace.activeRoot, openFolder, (session) => {
        recordBrowseTiming("workspace-root-ready", { root: session.rootPath, elapsedMs: performance.now() - started });
        queryClient.setQueryData<FolderSession[]>(["open-folders"], (current = []) =>
          insertRestoredFolder(current, session, roots));
      }, setFolderRestoreStates);
      // Do not replay completed results: users may have removed a ready root.
      return queryClient.getQueryData<FolderSession[]>(["open-folders"]) ?? [];
    },
    retry: false,
    // The perf harness opens its scenario folder explicitly; restoring library
    // roots would add noise from previously recorded runs.
    enabled: !perfScenario,
    staleTime: Infinity,
  });
  const sessions = foldersQuery.data ?? [];
  const folderSort = workspace.folderSort ?? "import";
  const folderDragEnabled = workspace.folderDragEnabled ?? false;
  const sortedSessions = useMemo(
    () => sortFolderSessions(sessions, folderSort, locale),
    [folderSort, locale, sessions],
  );
  const activeRootRestoring = folderRestoreStates.some((state) =>
    state.rootPath === workspace.activeRoot && state.status === "restoring");
  const restoringFolders = foldersQuery.isLoading || folderRestoreStates.some((state) =>
    state.status === "restoring");
  const activeSession = sessions.find((item) => item.rootPath === workspace.activeRoot) ??
    (activeRootRestoring ? undefined : sortedSessions[0]);
  useEffect(() => {
    if (activeSession && activeSession.rootPath !== workspace.activeRoot) {
      setWorkspace((current) => ({ ...current, activeRoot: activeSession.rootPath }));
    }
  }, [activeSession, workspace.activeRoot]);
  const currentPath = activeSession
    ? workspace.currentDirectories[activeSession.rootPath] ?? activeSession.rootPath
    : undefined;
  const browseTarget = useRef({ sessionId: activeSession?.id, directory: currentPath });
  browseTarget.current = { sessionId: activeSession?.id, directory: currentPath };
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onDirectoryBrowseProgress((progress) => {
      recordBrowseTiming("native-browse", { ...progress });
      if (progress.sessionId === browseTarget.current.sessionId && progress.directory === browseTarget.current.directory) {
        setBrowseProgress(progress);
      }
      if (progress.stage === "updated") {
        for (const key of ["assets", "preload-assets", "progressive-metadata-assets"]) {
          void queryClient.invalidateQueries({ queryKey: [key, progress.sessionId, progress.directory] });
        }
      }
    }).then((dispose) => { if (disposed) dispose(); else unlisten = dispose; });
    return () => { disposed = true; unlisten?.(); };
  }, [queryClient]);

  useEffect(() => {
    if (activeSession && currentPath) {
      setBrowserImageResourceScope(`${activeSession.id}\0${currentPath}`);
    }
  }, [activeSession, currentPath]);

  const notifyActiveDirectory = useCallback((session: FolderSession, path: string) => {
    const noticeKey = `${session.id}\0${path}`;
    if (activeDirectoryNoticeRef.current === noticeKey) return;
    activeDirectoryNoticeRef.current = noticeKey;
    void setActiveDirectory(session.id, path).catch((cause) => {
      if (activeDirectoryNoticeRef.current === noticeKey) {
        activeDirectoryNoticeRef.current = undefined;
        setWorkspace((current) =>
          recoverMissingCurrentDirectory(current, session.rootPath, path)
        );
      }
      if (__OXY_DEBUG__) console.warn("Active directory no longer exists", cause);
    });
  }, []);

  useEffect(() => saveWorkspace(workspace), [workspace]);

  useEffect(() => {
    if (activeSession && currentPath) notifyActiveDirectory(activeSession, currentPath);
  }, [activeSession?.id, currentPath, notifyActiveDirectory]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onLibraryDirectoryIndexUpdated(() => {
      void queryClient.invalidateQueries({ queryKey: ["directory-search"] });
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queryClient]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onLibraryIndexUpdated((update) => {
      setFolderImport((state) => applyIndexCompleted(state, update));
      void queryClient.invalidateQueries({ queryKey: ["assets"] });
      void queryClient.invalidateQueries({ queryKey: ["directory-search"] });
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queryClient]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onDirectoryTreeUpdated((snapshot) => {
      queryClient.setQueryData<DirectoryTreeSnapshot>(
        ["directory-tree", snapshot.sessionId],
        (current) => acceptDirectoryTreeSnapshot(current, snapshot),
      );
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queryClient]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onMetadataProjectionUpdated(queueMetadataProjection).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onImageProjectionUpdated(acceptImageProjection).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const query = useMemo<AssetQuery>(() => ({
    search: search || undefined,
    tagIds: tagIds.length ? tagIds : undefined,
    tagMatch,
    kind,
    minimumRating,
    colorLabels: colorLabels.length ? colorLabels : undefined,
    pickLabels: pickLabels.length ? pickLabels : undefined,
    sort,
    direction,
    pageSize: 250,
  }), [colorLabels, direction, kind, minimumRating, pickLabels, search, sort, tagIds, tagMatch]);
  const metadataFiltersActive = Boolean(minimumRating || colorLabels.length || pickLabels.length);
  if (metadataFiltersActive && activeId) filteredFocusRef.current = activeId;
  const filteredFocusRestoreId = metadataFiltersActive ? undefined : filteredFocusRef.current;
  const progressivelyFilterMetadata = Boolean(!search && !tagIds.length && (minimumRating || colorLabels.length || pickLabels.length));
  const metadataRecords = useMetadataProjectionStore((state) => progressivelyFilterMetadata ? state.records : NO_METADATA_RECORDS);
  const shouldPreloadFilteredAssets = Boolean(tagIds.length || search || kind || minimumRating || colorLabels.length || pickLabels.length);
  const preloadQuery = useMemo<AssetQuery>(() => ({
    sort: "name",
    direction: "ascending",
    pageSize: 250,
  }), []);
  const progressiveMetadataBatchQuery = useMemo<AssetQuery>(() => ({
    ...preloadQuery,
    pageSize: 32,
  }), [preloadQuery]);

  const assetsQuery = useInfiniteQuery({
    queryKey: ["assets", activeSession?.id, currentPath, query],
    queryFn: ({ pageParam }) => listAssets(activeSession!.id, currentPath!, query, pageParam.offset, pageParam.snapshotRevision),
    initialPageParam: firstBrowseCursor,
    getNextPageParam: nextBrowseCursor,
    enabled: Boolean(activeSession && currentPath && !progressivelyFilterMetadata),
    staleTime: Infinity,
  });
  const preloadAssetsQuery = useInfiniteQuery({
    queryKey: ["preload-assets", activeSession?.id, currentPath],
    queryFn: ({ pageParam }) => listAssets(
      activeSession!.id,
      currentPath!,
      preloadQuery,
      pageParam.offset,
      pageParam.snapshotRevision,
    ),
    initialPageParam: firstBrowseCursor,
    getNextPageParam: nextBrowseCursor,
    enabled: Boolean(
      shouldPreloadFilteredAssets &&
      !assetsQuery.isLoading &&
      !progressivelyFilterMetadata &&
      activeSession &&
      currentPath,
    ),
    staleTime: Infinity,
  });
  const progressiveMetadataQuery = useInfiniteQuery({
    queryKey: ["progressive-metadata-assets", activeSession?.id, currentPath],
    queryFn: async ({ pageParam }) => {
      const page = await listAssets(
        activeSession!.id,
        currentPath!,
        progressiveMetadataBatchQuery,
        pageParam.offset,
        pageParam.snapshotRevision,
      );
      const snapshots = await requestMetadata(page.items.map((asset) => asset.path), "filter");
      acceptMetadataProjections(snapshots);
      return page;
    },
    initialPageParam: firstBrowseCursor,
    getNextPageParam: nextBrowseCursor,
    enabled: Boolean(progressivelyFilterMetadata && activeSession && currentPath),
    staleTime: Infinity,
  });
  const resultScope = `${activeSession?.id}/${currentPath}`;
  const [lastResult, setLastResult] = useState<{ scope: string; data: typeof assetsQuery.data }>();
  useEffect(() => {
    if (assetsQuery.data) setLastResult({ scope: resultScope, data: assetsQuery.data });
  }, [assetsQuery.data, resultScope]);
  const resultData = assetsQuery.data ?? (lastResult?.scope === resultScope ? lastResult.data : undefined);
  const cheapAssets = useMemo(
    () => resultData?.pages.flatMap((page) => page.items) ?? [],
    [resultData],
  );
  const progressivelyEnrichedAssets = useMemo(
    () => progressiveMetadataQuery.data?.pages.flatMap((page) =>
      page.items.map((asset) => projectAssetMetadata(asset, metadataRecords[asset.path]))) ?? [],
    [metadataRecords, progressiveMetadataQuery.data],
  );
  const metadataProjectionPending = progressivelyEnrichedAssets.some((asset) => {
    const projection = metadataRecords[asset.path];
    return !projection || projection.status === "loading";
  });
  const assets = useMemo(
    () => progressivelyFilterMetadata
      ? filterAndSortAssets(progressivelyEnrichedAssets, query)
      : cheapAssets,
    [cheapAssets, progressivelyEnrichedAssets, progressivelyFilterMetadata, query],
  );
  // Page and metadata commits are non-urgent; keep active scrolling ahead of
  // rebuilding the browser projection for a newly returned page.
  const displayedAssets = useDeferredValue(assets);
  const preloadCandidates = useMemo(() => {
    const listed = progressivelyFilterMetadata ? assets : cheapAssets;
    const visibleIds = new Set(listed.map((asset) => asset.id));
    const candidates = progressivelyFilterMetadata
      ? progressiveMetadataQuery.data?.pages.flatMap((page) => page.items) ?? []
      : preloadAssetsQuery.data?.pages.flatMap((page) => page.items) ?? [];
    return [...listed, ...candidates.filter((asset) => !visibleIds.has(asset.id))];
  }, [assets, cheapAssets, preloadAssetsQuery.data, progressiveMetadataQuery.data, progressivelyFilterMetadata]);
  const total = progressivelyFilterMetadata
    ? assets.length
    : resultData?.pages[0]?.total ?? 0;
  const activeAsset = assets.find((asset) => asset.id === activeId);
  // The status bar reports the selected photo's position in the visible order,
  // not how many thumbs happen to be paged in so far.
  const activeOrdinal = useMemo(
    () => activeAssetOrdinal(displayedAssets, activeId),
    [activeId, displayedAssets],
  );
  const progressiveWorkPending = progressivelyFilterMetadata && (
    progressiveMetadataQuery.isLoading ||
    progressiveMetadataQuery.isFetchingNextPage ||
    progressiveMetadataQuery.hasNextPage ||
    metadataProjectionPending
  );
  const assetsLoading = progressivelyFilterMetadata
    ? assets.length === 0 && progressiveWorkPending
    : assetsQuery.isLoading;
  const assetsError = progressivelyFilterMetadata
    ? progressiveMetadataQuery.error
    : assetsQuery.error;
  const fetchNextAssetsPage = useCallback(() => {
    if (progressivelyFilterMetadata) return;
    // Scroll events can race the query's fetching-state render. Repeated calls
    // must leave the active page request running instead of restarting it.
    void assetsQuery.fetchNextPage({ cancelRefetch: false });
  }, [assetsQuery.fetchNextPage, progressivelyFilterMetadata]);
  useBackgroundAssetPagination({
    scopeKey: JSON.stringify([activeSession?.id, currentPath, query]),
    enabled: Boolean(activeSession && currentPath && !progressivelyFilterMetadata),
    pageCount: assetsQuery.data?.pages.length ?? 0,
    hasNextPage: assetsQuery.hasNextPage,
    isFetching: assetsQuery.isFetching,
    isError: assetsQuery.isError,
    fetchNextPage: assetsQuery.fetchNextPage,
  });
  const filteredFocusAction = focusRestoreAction(
    assets,
    filteredFocusRestoreId,
    Boolean(assetsQuery.hasNextPage),
    assetsQuery.isFetchingNextPage,
  );
  useEffect(() => {
    if (!filteredFocusRestoreId) return;
    if (activeId !== filteredFocusRestoreId || filteredFocusAction === "none") {
      filteredFocusRef.current = undefined;
      return;
    }
    if (filteredFocusAction === "fetch") {
      fetchNextAssetsPage();
    } else if (filteredFocusAction === "fallback" && assets[0]) {
      filteredFocusRef.current = undefined;
      select(assets[0].id);
    }
  }, [activeId, assets, fetchNextAssetsPage, filteredFocusAction, filteredFocusRestoreId, select]);
  const currentBrowseProgress = browseProgress?.sessionId === activeSession?.id && browseProgress?.directory === currentPath
    ? browseProgress : assetsQuery.data?.pages[0]?.progress;
  const importSummary = folderImportSummary(folderImport);
  const importNotice: StatusNotice | undefined = folderImportNotice(folderImport, t);
  const notice: StatusNotice | undefined = error || foldersQuery.isError
    ? { kind: "error", message: error ?? String(foldersQuery.error), detail: undefined }
    : importNotice ?? (currentBrowseProgress?.stage === "stale"
      ? { kind: "status", message: t("browseSnapshotOffline"), detail: currentBrowseProgress.error }
      : currentBrowseProgress?.source === "snapshot"
        ? { kind: "status", message: t("browseSnapshotChecking"), detail: undefined }
        : undefined);
  // A clean result is transient; failures stay until dismissed so the reason is readable.
  const importSettledCleanly = folderImport.visible && !importSummary.pending
    && importSummary.failed === 0 && !folderImport.error;
  useEffect(() => {
    if (!importSettledCleanly) return;
    const timer = window.setTimeout(() => setFolderImport(dismissFolderImport()), IMPORT_NOTICE_MS);
    return () => window.clearTimeout(timer);
  }, [importSettledCleanly]);
  useEffect(() => {
    setNoticeExpanded(false);
  }, [notice?.message]);
  useEffect(() => {
    if (assetsLoading || !activeSession || !currentPath) return;
    const requested = performance.now();
    const frame = requestAnimationFrame(() => {
      recordBrowseTiming("first-page-render-frame", { directory: currentPath, afterCommitMs: performance.now() - requested, total });
    });
    return () => cancelAnimationFrame(frame);
  }, [activeSession?.id, currentPath, assetsLoading]);

  useBackgroundAssetPagination({
    scopeKey: JSON.stringify([activeSession?.id, currentPath]),
    enabled: shouldPreloadFilteredAssets && !progressivelyFilterMetadata,
    pageCount: preloadAssetsQuery.data?.pages.length ?? 0,
    hasNextPage: preloadAssetsQuery.hasNextPage,
    isFetching: preloadAssetsQuery.isFetching,
    isError: preloadAssetsQuery.isError,
    fetchNextPage: preloadAssetsQuery.fetchNextPage,
  });

  useEffect(() => {
    if (
      progressiveMetadataQuery.hasNextPage &&
      !progressiveMetadataQuery.isFetchingNextPage
    ) {
      void progressiveMetadataQuery.fetchNextPage();
    }
  }, [
    progressiveMetadataQuery.data?.pages.length,
    progressiveMetadataQuery.fetchNextPage,
    progressiveMetadataQuery.hasNextPage,
    progressiveMetadataQuery.isFetchingNextPage,
  ]);

  const dismissOnboarding = useCallback(() => {
    completeFolderOnboarding();
    setShowOnboarding(false);
  }, []);

  /**
   * Open one root and register it in the library. Registration is what schedules
   * the background index and it is idempotent, so re-adding an existing root is
   * safe. This variant throws so a multi-folder import can report per-root
   * failures instead of collapsing them into one error banner.
   */
  const registerRoot = useCallback(async (path: string, activate: boolean) => {
    const opened = await openFolder(path);
    if (!perfScenario) await addLibraryRoot(opened.rootPath);
    queryClient.setQueryData<FolderSession[]>(["open-folders"], (current = []) => {
      const existing = current.find((item) => item.rootPath === opened.rootPath);
      return existing ? current : [...current, opened];
    });
    setWorkspace((current) => ({
      ...current,
      activeRoot: activate ? opened.rootPath : current.activeRoot,
      currentDirectories: {
        ...current.currentDirectories,
        [opened.rootPath]: current.currentDirectories[opened.rootPath] ?? opened.rootPath,
      },
    }));
    return opened;
  }, [perfScenario, queryClient]);

  /**
   * Raw open, without the import affordance. Only the perf harness uses this:
   * its scenario folders must not be registered or narrated.
   */
  const openPath = useCallback(async (path: string) => {
    setError(undefined);
    try {
      clearSelection();
      await registerRoot(path, true);
      dismissOnboarding();
    } catch (cause) {
      setError(String(cause));
    }
  }, [clearSelection, dismissOnboarding, registerRoot]);

  /**
   * The one import pipeline, shared by the drop target and the folder picker:
   * open and register each folder in the given order — exactly what opening a
   * folder already did, plus progress narration. No resolution pass and no
   * analysis of how the paths relate to each other or to the library.
   */
  const startFolderImport = useCallback(async (paths: string[]) => {
    if (!paths.length) return;
    setError(undefined);
    setFolderImport(beginFolderImport(paths));
    clearSelection();
    let firstRoot: string | undefined;
    for (const [index, path] of paths.entries()) {
      try {
        const opened = await registerRoot(path, firstRoot === undefined);
        firstRoot ??= opened.rootPath;
        // The row is keyed by the canonical root the backend returned.
        setFolderImport((state) => attachImportRoot(state, index, opened.rootPath));
        setFolderImport((state) => applyEntryOpened(state, opened.rootPath));
        // The browser demo has no background indexer or event stream, so an
        // imported folder is settled here instead of leaving the row spinning.
        if (!isTauri()) {
          setFolderImport((state) =>
            applyIndexCompleted(state, { rootPath: opened.rootPath, assetCount: 0, directoryCount: 0 }));
        }
      } catch (cause) {
        setFolderImport((state) => applyEntryFailure(state, path, String(cause)));
      }
    }
    if (firstRoot) dismissOnboarding();
  }, [clearSelection, dismissOnboarding, registerRoot]);

  /** The "+" button uses the picker, then the same pipeline as a drop. */
  const handleOpen = useCallback(async () => {
    const path = await chooseFolder();
    if (path) await startFolderImport([path]);
  }, [startFolderImport]);

  const retryFolderImport = useCallback(async (path: string) => {
    try {
      const opened = await registerRoot(path, false);
      setFolderImport((state) => applyEntryRetried(state, opened.rootPath));
    } catch (cause) {
      setFolderImport((state) => applyEntryFailure(state, path, String(cause)));
    }
  }, [registerRoot]);

  const retryFailedImports = useCallback(async () => {
    for (const entry of failedImportEntries(folderImport)) {
      await retryFolderImport(entry.rootPath ?? entry.droppedPath);
    }
  }, [folderImport, retryFolderImport]);

  const dropState = useFolderDrop(startFolderImport);

  /** Dismissing a status-bar message also retires a finished import result. */
  const dismissStatusNotice = useCallback(() => {
    setError(undefined);
    setNoticeExpanded(false);
    setFolderImport(dismissFolderImport());
  }, []);

  const handleNavigate = useCallback((session: FolderSession, path: string) => {
    notifyActiveDirectory(session, path);
    clearSelection();
    setWorkspace((current) => ({
      ...current,
      activeRoot: session.rootPath,
      currentDirectories: { ...current.currentDirectories, [session.rootPath]: path },
    }));
  }, [clearSelection, notifyActiveDirectory]);

  const handleRemove = useCallback(async (session: FolderSession) => {
    setError(undefined);
    try {
      const roots = await removeLibraryRoot(session.rootPath);
      clearSelection();
      queryClient.setQueryData<FolderSession[]>(["open-folders"], (current = []) =>
        current.filter((item) => roots.includes(item.rootPath)),
      );
      setWorkspace((current) => {
        const currentDirectories = { ...current.currentDirectories };
        delete currentDirectories[session.rootPath];
        const activeRoot = current.activeRoot === session.rootPath
          ? roots[0]
          : current.activeRoot;
        return { ...current, activeRoot, currentDirectories };
      });
    } catch (cause) {
      setError(String(cause));
    }
  }, [clearSelection, queryClient]);

  const handleFolderSortChange = useCallback((nextSort: FolderSort) => {
    setWorkspace((current) => ({
      ...current,
      folderSort: nextSort,
      folderDragEnabled: nextSort === "import" ? current.folderDragEnabled : false,
    }));
  }, []);

  const handleFolderDragEnabledChange = useCallback((enabled: boolean) => {
    setWorkspace((current) => ({
      ...current,
      folderDragEnabled: enabled,
      folderSort: enabled ? "import" : current.folderSort,
    }));
  }, []);

  const handleReorderFolders = useCallback(async (rootPaths: string[]) => {
    setError(undefined);
    const previous = queryClient.getQueryData<FolderSession[]>(["open-folders"]);
    const byPath = new Map(previous?.map((session) => [session.rootPath, session]));
    queryClient.setQueryData<FolderSession[]>(
      ["open-folders"],
      rootPaths.flatMap((path) => {
        const session = byPath.get(path);
        return session ? [session] : [];
      }),
    );
    try {
      const allRoots = await listLibraryRoots();
      await reorderLibraryRoots(mergeVisibleFolderOrder(allRoots, rootPaths));
    } catch (cause) {
      queryClient.setQueryData(["open-folders"], previous);
      setError(String(cause));
    }
  }, [queryClient]);

  const handleRefresh = useCallback(async () => {
    if (!activeSession || !currentPath || isRefreshing) return;
    setError(undefined);
    setIsRefreshing(true);
    try {
      await Promise.all([
        queryClient.cancelQueries({ queryKey: ["assets", activeSession.id, currentPath] }),
        queryClient.cancelQueries({ queryKey: ["preload-assets", activeSession.id, currentPath] }),
        queryClient.cancelQueries({ queryKey: ["progressive-metadata-assets", activeSession.id, currentPath] }),
        queryClient.cancelQueries({ queryKey: ["directory-tree", activeSession.id] }),
      ]);
      const tree = await refreshDirectory(activeSession.id, currentPath);
      queryClient.setQueryData<DirectoryTreeSnapshot>(
        ["directory-tree", activeSession.id],
        (current) => acceptDirectoryTreeSnapshot(current, tree),
      );
      invalidateMetadataDirectory(currentPath);
      invalidateImageDirectory(currentPath);
      queryClient.removeQueries({ queryKey: ["asset-render"] });
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["assets", activeSession.id, currentPath] }),
        queryClient.invalidateQueries({ queryKey: ["preload-assets", activeSession.id, currentPath] }),
        queryClient.invalidateQueries({ queryKey: ["progressive-metadata-assets", activeSession.id, currentPath] }),
        queryClient.invalidateQueries({ queryKey: ["directory-search", activeSession.id] }),
      ]);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setIsRefreshing(false);
    }
  }, [activeSession, currentPath, isRefreshing, queryClient]);

  const handleTrashAsset = useCallback(async (asset: typeof assets[number]) => {
    setError(undefined);
    try {
      await deletePaths([asset.path], activeSession?.deletionMode ?? "trash");
      const replacementId = replacementAssetIdAfterRemoval(assets, asset.id);
      if (replacementId) select(replacementId);
      else clearSelection();
      queryClient.removeQueries({ queryKey: ["asset-render", asset.id] });
      await handleRefresh();
    } catch (cause) {
      setError(String(cause));
    }
  }, [activeSession?.deletionMode, assets, clearSelection, handleRefresh, queryClient, select]);

  const handleTrashFolder = useCallback(async (session: FolderSession, path: string) => {
    setError(undefined);
    try {
      await deletePaths([path], session.deletionMode);
      if (path === session.rootPath) {
        await handleRemove(session);
        return;
      }

      const parent = parentFolderPath(path);
      clearSelection();
      if (activeSession?.rootPath === session.rootPath && currentPath && isSameOrDescendantPath(path, currentPath)) {
        setWorkspace((current) => ({
          ...current,
          currentDirectories: { ...current.currentDirectories, [session.rootPath]: parent },
        }));
      }
      await queryClient.cancelQueries({ queryKey: ["directory-tree", session.id] });
      const tree = await refreshDirectory(session.id, parent);
      queryClient.setQueryData<DirectoryTreeSnapshot>(
        ["directory-tree", session.id],
        (current) => acceptDirectoryTreeSnapshot(current, tree),
      );
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["assets", session.id] }),
        queryClient.invalidateQueries({ queryKey: ["preload-assets", session.id] }),
        queryClient.invalidateQueries({ queryKey: ["progressive-metadata-assets", session.id] }),
        queryClient.invalidateQueries({ queryKey: ["directory-search", session.id] }),
      ]);
    } catch (cause) {
      setError(String(cause));
    }
  }, [activeSession?.rootPath, clearSelection, currentPath, handleRemove, queryClient]);

  const handleCopyPath = useCallback(async (
    rootPath: string,
    path: string,
    relative: boolean,
  ) => {
    setError(undefined);
    try {
      await copyText(relative ? relativeFolderPath(rootPath, path) : path);
    } catch (cause) {
      setError(String(cause));
    }
  }, []);

  const handleOpenExternal = useCallback(async (path: string, appId?: string) => {
    setError(undefined);
    try {
      if (!isTauri()) throw new Error(t("externalDesktopOnly"));
      if (appId) await openAssetWithApplication(path, appId);
      else await openAssetWithSystemDialog(path);
    } catch (cause) { setError(String(cause)); }
  }, [t]);

  const handleOpenInFileManager = useCallback(async (path: string) => {
    setError(undefined);
    try {
      await openInFileManager(path);
    } catch (cause) {
      setError(String(cause));
    }
  }, []);

  return (
    <div
      ref={appShellRef}
      className={`app-shell ${leftPanelOpen ? "" : "sidebar-collapsed"} ${inspectorOpen ? "" : "inspector-collapsed"}`}
      style={{
        "--left-panel-width": `${leftPanelWidth}px`,
        "--inspector-width": `${displayedInspectorWidth}px`,
      } as CSSProperties}
    >
      <Sidebar
        sessions={sortedSessions}
        folderRestoreStates={folderRestoreStates}
        activeSession={activeSession}
        currentPath={currentPath}
        showOnboarding={showOnboarding && sessions.length === 0 && !restoringFolders}
        onOpen={handleOpen}
        onNavigate={handleNavigate}
        onRemove={handleRemove}
        onTrashFolder={(session, path) => void handleTrashFolder(session, path)}
        onCopyFolderPath={(session, path, relative) => void handleCopyPath(session.rootPath, path, relative)}
        onOpenInFileManager={(path) => void handleOpenInFileManager(path)}
        onRefresh={handleRefresh}
        isRefreshing={isRefreshing}
        onDismissOnboarding={dismissOnboarding}
        onSettings={toggleSettings}
        folderSort={folderSort}
        onFolderSortChange={handleFolderSortChange}
        folderDragEnabled={folderDragEnabled && !restoringFolders}
        onFolderDragEnabledChange={handleFolderDragEnabledChange}
        onReorderFolders={(rootPaths) => void handleReorderFolders(rootPaths)}
        t={t}
      />
      <ResizeHandle
        axis="x"
        className="resize-handle--left"
        cssVariable="--left-panel-width"
        defaultValue={LAYOUT_SIZE_LIMITS.leftPanel.defaultValue}
        direction={1}
        label={t("resizeLeftPanel")}
        max={LAYOUT_SIZE_LIMITS.leftPanel.max}
        min={LAYOUT_SIZE_LIMITS.leftPanel.min}
        onCommit={setLeftPanelWidth}
        targetRef={appShellRef}
        value={leftPanelWidth}
      />
      <section className="workspace">
        {assetsError && resultData ? <div className="search-query-error" role="status">{String(assetsError)}
          <button onClick={() => void assetsQuery.refetch()}>{t("retry")}</button></div> : null}
        <Toolbar total={total} loading={assetsLoading || assetsQuery.isFetching || !activeSession && restoringFolders} t={t} />
        {!activeSession ? (
          <div className="workspace-empty">
            <FolderPlus size={29} strokeWidth={1.4} />
            <strong>{restoringFolders ? t("restoringFolders") : t("noFolderTitle")}</strong>
            <span>{restoringFolders ? t("restoringFoldersBody") : t("noFolderBody")}</span>
          </div>
        ) : assetsLoading && !resultData ? (
          <div className="workspace-loading"><Aperture size={24} /> {t("scanningFolder")} {currentPath?.split(/[\\/]/).pop() || activeSession.displayName}…
            {currentBrowseProgress && <span>{t("browseDiscovered")} {currentBrowseProgress.discoveredCount.toLocaleString()} {t("photos")}</span>}
          </div>
        ) : assetsError && !resultData ? (
          <div className="workspace-error is-selectable">
            <CircleAlert size={24} />
            <strong>{String(assetsError)}</strong>
            <button onClick={handleOpen}><FolderPlus size={15} />{t("openFolder")}</button>
          </div>
        ) : !displayedAssets.length && !assetsQuery.isFetching && !assetsError && (search || tagIds.length) ? (
          <div className="workspace-empty"><strong>{t("noSearchResults")}</strong>
            <button onClick={clearSearch}>{t("clearSearchTags")}</button></div>
        ) : (
          <AssetBrowser
            assets={displayedAssets}
            deletionMode={activeSession.deletionMode}
            restoringActiveId={filteredFocusAction === "none" ? undefined : filteredFocusRestoreId}
            total={total}
            view={view}
            hasNextPage={progressivelyFilterMetadata ? false : assetsQuery.hasNextPage}
            isFetchingNextPage={progressivelyFilterMetadata
              ? progressiveWorkPending
              : assetsQuery.isFetchingNextPage}
            fetchNextPage={fetchNextAssetsPage}
            onTrashAsset={(asset) => void handleTrashAsset(asset)}
            onCopyAssetPath={(asset, relative) => void handleCopyPath(activeSession.rootPath, asset.path, relative)}
            onOpenExternal={handleOpenExternal}
            onOpenInFileManager={(path) => void handleOpenInFileManager(path)}
            t={t}
          />
        )}
        <footer className="statusbar">
          <span title={currentPath}><i className="status-dot" /> {
            currentPath?.split(/[\\/]/).filter(Boolean).at(-1) ?? t("noFolderOpen")
          }</span>
          {notice ? (
            <span className={`statusbar__notice statusbar__notice--${notice.kind}`}>
              <button
                className="statusbar__notice-toggle"
                onClick={() => setNoticeExpanded((expanded) => !expanded)}
                aria-expanded={noticeExpanded}
                title={notice.message}
              >
                {notice.kind === "error" ? <CircleAlert size={11} /> : null}
                <span>{notice.message}</span>
              </button>
              {noticeExpanded ? (
                <span className="statusbar__notice-panel is-selectable" role="status">
                  <span>{notice.message}</span>
                  {notice.detail ? <span className="statusbar__notice-detail">{notice.detail}</span> : null}
                  {notice.retry ? (
                    <button
                      className="statusbar__notice-action"
                      onClick={() => void retryFailedImports()}
                    >{t("importRetry")}</button>
                  ) : null}
                  {notice.kind === "error" ? (
                    <button
                      className="statusbar__notice-dismiss"
                      onClick={dismissStatusNotice}
                      aria-label={t("dismissNotice")}
                      title={t("dismissNotice")}
                    >×</button>
                  ) : null}
                </span>
              ) : null}
            </span>
          ) : null}
          <span>{assetsLoading || !activeSession && restoringFolders ? t("loading") : `${activeOrdinal === undefined ? "–" : activeOrdinal.toLocaleString()} / ${total.toLocaleString()} ${t("photos")}`}</span>
          <span
            className="statusbar__thumbnail-orientation"
            role="group"
            aria-label={t("thumbnailOrientation")}
          >
            <button
              className={thumbnailOrientation === "landscape" ? "is-active" : ""}
              onClick={() => setThumbnailOrientation("landscape")}
              title={t("landscapePriority")}
              aria-label={t("landscapePriority")}
            >
              <RectangleHorizontal size={14} />
            </button>
            <button
              className={thumbnailOrientation === "portrait" ? "is-active" : ""}
              onClick={() => setThumbnailOrientation("portrait")}
              title={t("portraitPriority")}
              aria-label={t("portraitPriority")}
            >
              <RectangleVertical size={14} />
            </button>
          </span>
          <span>{selectedIds.length} {t("selected")}</span>
        </footer>
      </section>
      <ResizeHandle
        axis="x"
        className="resize-handle--right"
        cssVariable="--inspector-width"
        defaultValue={LAYOUT_SIZE_LIMITS.inspector.defaultValue}
        direction={-1}
        label={t("resizeInspector")}
        max={inspectorMax}
        min={LAYOUT_SIZE_LIMITS.inspector.min}
        onCommit={setInspectorWidth}
        targetRef={appShellRef}
        value={displayedInspectorWidth}
      />
      <Inspector
        asset={activeAsset}
        selectedPaths={assets.filter((asset) => selectedIds.includes(asset.id)).map((asset) => asset.path)}
        selectedCount={selectedIds.length}
        t={t}
      />
      {(progressivelyFilterMetadata ? progressiveMetadataQuery.isSuccess : assetsQuery.isSuccess) &&
      activeSession && currentPath ? (
        <BackgroundPreviewPreloader
          key={`${activeSession.id}:${currentPath}`}
          assets={preloadCandidates}
        />
      ) : null}
      {!isTauri() ? <span className="demo-pill">{t("demoHint")}</span> : null}
      <ImportOverlay
        visible={dropState.visible}
        folderNames={dropState.folderNames}
        itemCount={dropState.itemCount}
        t={t}
      />
      {settingsOpen ? <SettingsPanel t={t} activeAsset={assets.find((asset) => asset.id === activeId)} /> : null}
      {perfScenario ? (
        <PerfHarness
          scenario={perfScenario}
          session={activeSession}
          assets={assets}
          assetsLoading={assetsQuery.isLoading}
          onOpenPath={openPath}
        />
      ) : null}
    </div>
  );
}
