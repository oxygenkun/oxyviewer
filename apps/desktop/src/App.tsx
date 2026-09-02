import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { Aperture, CircleAlert, FolderPlus, RectangleHorizontal, RectangleVertical } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { AssetBrowser } from "./components/AssetBrowser";
import { BackgroundPreviewPreloader } from "./components/BackgroundPreviewPreloader";
import { Inspector } from "./components/Inspector";
import { PerfHarness } from "./components/PerfHarness";
import { SettingsPanel } from "./components/SettingsPanel";
import { Sidebar } from "./components/Sidebar";
import { Toolbar } from "./components/Toolbar";
import {
  addLibraryRoot,
  chooseFolder,
  isTauri,
  listAssets,
  listLibraryRoots,
  openFolder,
  refreshDirectory,
  removeLibraryRoot,
} from "./lib/api";
import { translate } from "./lib/i18n";
import {
  completeFolderOnboarding,
  hasSeenFolderOnboarding,
  loadWorkspace,
  saveWorkspace,
} from "./lib/workspacePersistence";
import { useWorkspaceStore } from "./store";
import type { AssetQuery, FolderSession, PerfScenario } from "./types";

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
  const {
    view, gridPreference, activeId, selectedIds, inspectorOpen, leftPanelOpen, settingsOpen, locale,
    search, kind, minimumRating, colorLabel, sort, direction, clearSelection, setGridPreference, toggleSettings,
  } = useWorkspaceStore();
  const t = useCallback((key: Parameters<typeof translate>[1]) => translate(locale, key), [locale]);

  const foldersQuery = useQuery({
    queryKey: ["open-folders"],
    queryFn: restoreFolders,
    // The perf harness opens its scenario folder explicitly; restoring library
    // roots would add noise from previously recorded runs.
    enabled: !perfScenario,
    staleTime: Infinity,
  });
  const sessions = foldersQuery.data ?? [];
  const activeSession = sessions.find((item) => item.rootPath === workspace.activeRoot) ?? sessions[0];
  const currentPath = activeSession
    ? workspace.currentDirectories[activeSession.rootPath] ?? activeSession.rootPath
    : undefined;

  useEffect(() => saveWorkspace(workspace), [workspace]);

  const query = useMemo<AssetQuery>(() => ({
    search: search || undefined,
    kind,
    minimumRating,
    colorLabel,
    sort,
    direction,
    pageSize: 250,
  }), [colorLabel, direction, kind, minimumRating, search, sort]);
  const filtersActive = Boolean(search || kind || minimumRating || colorLabel);
  const preloadQuery = useMemo<AssetQuery>(() => ({
    sort: "name",
    direction: "ascending",
    pageSize: 250,
  }), []);

  const assetsQuery = useInfiniteQuery({
    queryKey: ["assets", activeSession?.id, currentPath, query],
    queryFn: ({ pageParam }) => listAssets(activeSession!.id, currentPath!, query, pageParam),
    initialPageParam: 0,
    getNextPageParam: (page) => page.nextCursor,
    enabled: Boolean(activeSession && currentPath),
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
    enabled: Boolean(filtersActive && activeSession && currentPath),
    staleTime: Infinity,
  });
  const assets = useMemo(
    () => assetsQuery.data?.pages.flatMap((page) => page.items) ?? [],
    [assetsQuery.data],
  );
  const preloadCandidates = useMemo(() => {
    const visibleIds = new Set(assets.map((asset) => asset.id));
    return preloadAssetsQuery.data?.pages
      .flatMap((page) => page.items)
      .filter((asset) => !visibleIds.has(asset.id)) ?? [];
  }, [assets, preloadAssetsQuery.data]);
  const total = assetsQuery.data?.pages[0]?.total ?? 0;
  const activeAsset = assets.find((asset) => asset.id === activeId);

  useEffect(() => {
    if (preloadAssetsQuery.hasNextPage && !preloadAssetsQuery.isFetchingNextPage) {
      void preloadAssetsQuery.fetchNextPage();
    }
  }, [
    preloadAssetsQuery.data?.pages.length,
    preloadAssetsQuery.fetchNextPage,
    preloadAssetsQuery.hasNextPage,
    preloadAssetsQuery.isFetchingNextPage,
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
    clearSelection();
    setWorkspace((current) => ({
      activeRoot: session.rootPath,
      currentDirectories: { ...current.currentDirectories, [session.rootPath]: path },
    }));
  }, [clearSelection]);

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
        return { activeRoot, currentDirectories };
      });
    } catch (cause) {
      setError(String(cause));
    }
  }, [clearSelection, queryClient]);

  const handleRefresh = useCallback(async () => {
    if (!activeSession || !currentPath || isRefreshing) return;
    setError(undefined);
    setIsRefreshing(true);
    try {
      await Promise.all([
        queryClient.cancelQueries({ queryKey: ["assets", activeSession.id, currentPath] }),
        queryClient.cancelQueries({ queryKey: ["preload-assets", activeSession.id, currentPath] }),
        queryClient.cancelQueries({ queryKey: ["directories", activeSession.id, currentPath] }),
      ]);
      await refreshDirectory(activeSession.id, currentPath);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["assets", activeSession.id, currentPath] }),
        queryClient.invalidateQueries({ queryKey: ["preload-assets", activeSession.id, currentPath] }),
        queryClient.invalidateQueries({ queryKey: ["directories", activeSession.id, currentPath] }),
      ]);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setIsRefreshing(false);
    }
  }, [activeSession, currentPath, isRefreshing, queryClient]);

  return (
    <div
      className={`app-shell ${leftPanelOpen ? "" : "sidebar-collapsed"} ${inspectorOpen ? "" : "inspector-collapsed"}`}
    >
      <Sidebar
        sessions={sessions}
        activeSession={activeSession}
        currentPath={currentPath}
        showOnboarding={showOnboarding && sessions.length === 0 && !foldersQuery.isLoading}
        onOpen={handleOpen}
        onNavigate={handleNavigate}
        onRemove={handleRemove}
        onRefresh={handleRefresh}
        isRefreshing={isRefreshing}
        onDismissOnboarding={dismissOnboarding}
        onSettings={toggleSettings}
        t={t}
      />
      <section className="workspace">
        <Toolbar total={total} t={t} />
        {!activeSession ? (
          <div className="workspace-empty">
            <FolderPlus size={29} strokeWidth={1.4} />
            <strong>{foldersQuery.isLoading ? t("restoringFolders") : t("noFolderTitle")}</strong>
            <span>{foldersQuery.isLoading ? t("restoringFoldersBody") : t("noFolderBody")}</span>
          </div>
        ) : assetsQuery.isLoading ? (
          <div className="workspace-loading"><Aperture size={24} /> {t("scanningFolder")} {activeSession.displayName}…</div>
        ) : assetsQuery.isError ? (
          <div className="workspace-error">
            <CircleAlert size={24} />
            <strong>{String(assetsQuery.error)}</strong>
            <button onClick={handleOpen}><FolderPlus size={15} />{t("openFolder")}</button>
          </div>
        ) : (
          <AssetBrowser
            assets={assets}
            total={total}
            view={view}
            hasNextPage={assetsQuery.hasNextPage}
            isFetchingNextPage={assetsQuery.isFetchingNextPage}
            fetchNextPage={() => void assetsQuery.fetchNextPage()}
            t={t}
          />
        )}
        <footer className="statusbar">
          <span title={currentPath}><i className="status-dot" /> {
            currentPath?.split(/[\\/]/).filter(Boolean).at(-1) ?? t("noFolderOpen")
          }</span>
          <span>{assets.length.toLocaleString()} / {total.toLocaleString()} {t("photos")}</span>
          {view === "grid" ? (
            <span className="statusbar__grid-preference" role="group" aria-label={t("gridPreference")}>
              <button
                className={gridPreference === "landscape" ? "is-active" : ""}
                onClick={() => setGridPreference("landscape")}
                title={t("landscapePriority")}
                aria-label={t("landscapePriority")}
              >
                <RectangleHorizontal size={11} />
              </button>
              <button
                className={gridPreference === "portrait" ? "is-active" : ""}
                onClick={() => setGridPreference("portrait")}
                title={t("portraitPriority")}
                aria-label={t("portraitPriority")}
              >
                <RectangleVertical size={11} />
              </button>
            </span>
          ) : null}
          <span>{selectedIds.length} {t("selected")}</span>
        </footer>
      </section>
      <Inspector
        asset={activeAsset}
        selectedPaths={assets.filter((asset) => selectedIds.includes(asset.id)).map((asset) => asset.path)}
        selectedCount={selectedIds.length}
        t={t}
      />
      {filtersActive && assetsQuery.isSuccess && activeSession && currentPath ? (
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
