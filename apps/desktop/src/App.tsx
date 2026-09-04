import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { Aperture, CircleAlert, FolderPlus, RectangleHorizontal, RectangleVertical } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { AssetBrowser } from "./components/AssetBrowser";
import { BackgroundPreviewPreloader } from "./components/BackgroundPreviewPreloader";
import { Inspector } from "./components/Inspector";
import { PerfHarness } from "./components/PerfHarness";
import { SettingsPanel } from "./components/SettingsPanel";
import { Sidebar } from "./components/Sidebar";
import { ResizeHandle } from "./components/ResizeHandle";
import { Toolbar } from "./components/Toolbar";
import {
  addLibraryRoot,
  chooseFolder,
  copyText,
  isTauri,
  listAssets,
  listLibraryRoots,
  openFolder,
  openInFileManager,
  onDirectoryTreeUpdated,
  onLibraryIndexUpdated,
  onImageProjectionUpdated,
  onMetadataProjectionUpdated,
  refreshDirectory,
  requestMetadata,
  removeLibraryRoot,
  reorderLibraryRoots,
  setActiveDirectory,
  trashPaths,
} from "./lib/api";
import { filterAndSortAssets } from "./lib/assetFiltering";
import { replacementAssetIdAfterRemoval } from "./lib/assetViewPosition";
import { setBrowserImageResourceScope } from "./lib/browserImageCache";
import { acceptDirectoryTreeSnapshot } from "./lib/directoryTreeProjection";
import { acceptImageProjection, invalidateImageDirectory } from "./lib/imageProjection";
import {
  acceptMetadataProjection,
  invalidateMetadataDirectory,
  projectAssetMetadata,
  useMetadataProjectionStore,
} from "./lib/metadataProjection";
import { isSameOrDescendantPath, parentFolderPath, relativeFolderPath } from "./lib/folderPaths";
import { translate } from "./lib/i18n";
import { LAYOUT_SIZE_LIMITS } from "./lib/layoutSizing";
import {
  mergeVisibleFolderOrder,
  sortFolderSessions,
  type FolderSort,
} from "./lib/folderOrdering";
import {
  completeFolderOnboarding,
  hasSeenFolderOnboarding,
  loadWorkspace,
  recoverMissingCurrentDirectory,
  saveWorkspace,
} from "./lib/workspacePersistence";
import { useWorkspaceStore } from "./store";
import type { AssetQuery, DirectoryTreeSnapshot, FolderSession, PerfScenario } from "./types";

async function restoreFolders(): Promise<FolderSession[]> {
  const roots = await listLibraryRoots();
  const restored = await Promise.allSettled(roots.map((root) => openFolder(root)));
  return restored.flatMap((result) => result.status === "fulfilled" ? [result.value] : []);
}

export function App({ perfScenario }: { perfScenario?: PerfScenario }) {
  const [workspace, setWorkspace] = useState(loadWorkspace);
  const [showOnboarding, setShowOnboarding] = useState(() => !hasSeenFolderOnboarding());
  const [error, setError] = useState<string>();
  const [isRefreshing, setIsRefreshing] = useState(false);
  const queryClient = useQueryClient();
  const metadataRecords = useMetadataProjectionStore((state) => state.records);
  const {
    view, thumbnailOrientation, activeId, selectedIds, inspectorOpen, leftPanelOpen, settingsOpen, locale,
    search, kind, minimumRating, colorLabels, sort, direction, clearSelection, select, setThumbnailOrientation, toggleSettings,
    leftPanelWidth, inspectorWidth, setLeftPanelWidth, setInspectorWidth, uiFontScale,
  } = useWorkspaceStore();
  const appShellRef = useRef<HTMLDivElement>(null);
  const activeDirectoryNoticeRef = useRef<string | undefined>(undefined);
  const t = useCallback((key: Parameters<typeof translate>[1]) => translate(locale, key), [locale]);

  useLayoutEffect(() => {
    document.documentElement.style.fontSize = `${uiFontScale * 100}%`;
    return () => {
      document.documentElement.style.removeProperty("font-size");
    };
  }, [uiFontScale]);

  const foldersQuery = useQuery({
    queryKey: ["open-folders"],
    queryFn: restoreFolders,
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
  const activeSession = sessions.find((item) => item.rootPath === workspace.activeRoot) ??
    sortedSessions[0];
  const currentPath = activeSession
    ? workspace.currentDirectories[activeSession.rootPath] ?? activeSession.rootPath
    : undefined;

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
    void onLibraryIndexUpdated(() => {
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
    void onMetadataProjectionUpdated(acceptMetadataProjection).then((dispose) => {
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
    kind,
    minimumRating,
    colorLabels: colorLabels.length ? colorLabels : undefined,
    sort,
    direction,
    pageSize: 250,
  }), [colorLabels, direction, kind, minimumRating, search, sort]);
  const progressivelyFilterMetadata = Boolean(!search && (minimumRating || colorLabels.length));
  const shouldPreloadFilteredAssets = Boolean(!search && (kind || minimumRating || colorLabels.length));
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
    queryFn: ({ pageParam }) => listAssets(activeSession!.id, currentPath!, query, pageParam),
    initialPageParam: 0,
    getNextPageParam: (page) => page.nextCursor,
    enabled: Boolean(activeSession && currentPath && !progressivelyFilterMetadata),
    staleTime: Infinity,
  });
  const preloadAssetsQuery = useInfiniteQuery({
    queryKey: ["preload-assets", activeSession?.id, currentPath],
    queryFn: ({ pageParam }) => listAssets(
      activeSession!.id,
      currentPath!,
      preloadQuery,
      pageParam,
    ),
    initialPageParam: 0,
    getNextPageParam: (page) => page.nextCursor,
    enabled: Boolean(
      shouldPreloadFilteredAssets &&
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
        pageParam,
      );
      const snapshots = await requestMetadata(page.items.map((asset) => asset.path), "filter");
      snapshots.forEach(acceptMetadataProjection);
      return page;
    },
    initialPageParam: 0,
    getNextPageParam: (page) => page.nextCursor,
    enabled: Boolean(progressivelyFilterMetadata && activeSession && currentPath),
    staleTime: Infinity,
  });
  const cheapAssets = useMemo(
    () => assetsQuery.data?.pages.flatMap((page) => page.items) ?? [],
    [assetsQuery.data],
  );
  useEffect(() => {
    if (progressivelyFilterMetadata || cheapAssets.length === 0) return;
    void requestMetadata(cheapAssets.map((asset) => asset.path), "visible")
      .then((snapshots) => snapshots.forEach(acceptMetadataProjection))
      .catch(() => undefined);
  }, [cheapAssets, progressivelyFilterMetadata]);

  const enrichedAssets = useMemo(() => {
    return cheapAssets.map((asset) => projectAssetMetadata(asset, metadataRecords[asset.path]));
  }, [cheapAssets, metadataRecords]);
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
      : enrichedAssets,
    [enrichedAssets, progressivelyEnrichedAssets, progressivelyFilterMetadata, query],
  );
  const preloadCandidates = useMemo(() => {
    const visibleIds = new Set(assets.map((asset) => asset.id));
    const candidates = progressivelyFilterMetadata
      ? progressivelyEnrichedAssets
      : preloadAssetsQuery.data?.pages.flatMap((page) => page.items) ?? [];
    return candidates.filter((asset) => !visibleIds.has(asset.id));
  }, [assets, preloadAssetsQuery.data, progressivelyEnrichedAssets, progressivelyFilterMetadata]);
  const total = progressivelyFilterMetadata
    ? assets.length
    : assetsQuery.data?.pages[0]?.total ?? 0;
  const activeAsset = assets.find((asset) => asset.id === activeId);
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

  useEffect(() => {
    if (!preloadAssetsQuery.hasNextPage || preloadAssetsQuery.isFetchingNextPage) return;
    void preloadAssetsQuery.fetchNextPage();
  }, [
    preloadAssetsQuery.data?.pages.length,
    preloadAssetsQuery.fetchNextPage,
    preloadAssetsQuery.hasNextPage,
    preloadAssetsQuery.isFetchingNextPage,
  ]);

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

  const openPath = useCallback(async (path: string) => {
    setError(undefined);
    try {
      const opened = await openFolder(path);
      if (!perfScenario) await addLibraryRoot(opened.rootPath);
      clearSelection();
      queryClient.setQueryData<FolderSession[]>(["open-folders"], (current = []) => {
        const existing = current.find((item) => item.rootPath === opened.rootPath);
        return existing ? current : [...current, opened];
      });
      setWorkspace((current) => ({
        ...current,
        activeRoot: opened.rootPath,
        currentDirectories: {
          ...current.currentDirectories,
          [opened.rootPath]: current.currentDirectories[opened.rootPath] ?? opened.rootPath,
        },
      }));
      dismissOnboarding();
    } catch (cause) {
      setError(String(cause));
    }
  }, [clearSelection, dismissOnboarding, perfScenario, queryClient]);

  const handleOpen = useCallback(async () => {
    const path = await chooseFolder();
    if (path) await openPath(path);
  }, [openPath]);

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
      await trashPaths([asset.path]);
      const replacementId = replacementAssetIdAfterRemoval(assets, asset.id);
      if (replacementId) select(replacementId);
      else clearSelection();
      queryClient.removeQueries({ queryKey: ["asset-render", asset.id] });
      await handleRefresh();
    } catch (cause) {
      setError(String(cause));
    }
  }, [assets, clearSelection, handleRefresh, queryClient, select]);

  const handleTrashFolder = useCallback(async (session: FolderSession, path: string) => {
    setError(undefined);
    try {
      await trashPaths([path]);
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
        "--inspector-width": `${inspectorWidth}px`,
      } as CSSProperties}
    >
      <Sidebar
        sessions={sortedSessions}
        activeSession={activeSession}
        currentPath={currentPath}
        showOnboarding={showOnboarding && sessions.length === 0 && !foldersQuery.isLoading}
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
        folderDragEnabled={folderDragEnabled}
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
        <Toolbar total={total} t={t} />
        {!activeSession ? (
          <div className="workspace-empty">
            <FolderPlus size={29} strokeWidth={1.4} />
            <strong>{foldersQuery.isLoading ? t("restoringFolders") : t("noFolderTitle")}</strong>
            <span>{foldersQuery.isLoading ? t("restoringFoldersBody") : t("noFolderBody")}</span>
          </div>
        ) : assetsLoading ? (
          <div className="workspace-loading"><Aperture size={24} /> {t("scanningFolder")} {activeSession.displayName}…</div>
        ) : assetsError ? (
          <div className="workspace-error">
            <CircleAlert size={24} />
            <strong>{String(assetsError)}</strong>
            <button onClick={handleOpen}><FolderPlus size={15} />{t("openFolder")}</button>
          </div>
        ) : (
          <AssetBrowser
            assets={assets}
            total={total}
            view={view}
            hasNextPage={progressivelyFilterMetadata ? false : assetsQuery.hasNextPage}
            isFetchingNextPage={progressivelyFilterMetadata
              ? progressiveWorkPending
              : assetsQuery.isFetchingNextPage}
            fetchNextPage={() => {
              if (!progressivelyFilterMetadata) void assetsQuery.fetchNextPage();
            }}
            onTrashAsset={(asset) => void handleTrashAsset(asset)}
            onCopyAssetPath={(asset, relative) => void handleCopyPath(activeSession.rootPath, asset.path, relative)}
            onOpenInFileManager={(path) => void handleOpenInFileManager(path)}
            t={t}
          />
        )}
        <footer className="statusbar">
          <span title={currentPath}><i className="status-dot" /> {
            currentPath?.split(/[\\/]/).filter(Boolean).at(-1) ?? t("noFolderOpen")
          }</span>
          <span>{assets.length.toLocaleString()} / {total.toLocaleString()} {t("photos")}</span>
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
        max={LAYOUT_SIZE_LIMITS.inspector.max}
        min={LAYOUT_SIZE_LIMITS.inspector.min}
        onCommit={setInspectorWidth}
        targetRef={appShellRef}
        value={inspectorWidth}
      />
      <Inspector
        asset={activeAsset}
        selectedPaths={assets.filter((asset) => selectedIds.includes(asset.id)).map((asset) => asset.path)}
        selectedCount={selectedIds.length}
        t={t}
      />
      {shouldPreloadFilteredAssets &&
      (progressivelyFilterMetadata ? progressiveMetadataQuery.isSuccess : assetsQuery.isSuccess) &&
      activeSession && currentPath ? (
        <BackgroundPreviewPreloader
          key={`${activeSession.id}:${currentPath}`}
          assets={preloadCandidates}
        />
      ) : null}
      {error || foldersQuery.isError ? (
        <button className="error-toast" onClick={() => setError(undefined)}>
          <CircleAlert size={16} />{error ?? String(foldersQuery.error)}<span>×</span>
        </button>
      ) : null}
      {!isTauri() ? <span className="demo-pill">{t("demoHint")}</span> : null}
      {settingsOpen ? <SettingsPanel t={t} /> : null}
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
