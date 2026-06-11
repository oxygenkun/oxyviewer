import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { Aperture, CircleAlert, FolderOpen, RectangleHorizontal, RectangleVertical } from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import { AssetBrowser } from "./components/AssetBrowser";
import { EmptyState } from "./components/EmptyState";
import { Inspector } from "./components/Inspector";
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
} from "./lib/api";
import { translate } from "./lib/i18n";
import { useWorkspaceStore } from "./store";
import type { AssetQuery, FolderSession } from "./types";

export function App() {
  const [session, setSession] = useState<FolderSession>();
  const [currentPath, setCurrentPath] = useState<string>();
  const [error, setError] = useState<string>();
  const [isRefreshing, setIsRefreshing] = useState(false);
  const queryClient = useQueryClient();
  const {
    view, gridPreference, activeId, selectedIds, inspectorOpen, leftPanelOpen, locale,
    search, kind, sort, direction, clearSelection, setGridPreference,
  } = useWorkspaceStore();
  const t = useCallback((key: Parameters<typeof translate>[1]) => translate(locale, key), [locale]);

  const query = useMemo<AssetQuery>(() => ({
    search: search || undefined,
    kind,
    sort,
    direction,
    pageSize: 250,
  }), [direction, kind, search, sort]);

  const assetsQuery = useInfiniteQuery({
    queryKey: ["assets", session?.id, currentPath, query],
    queryFn: ({ pageParam }) => listAssets(session!.id, currentPath!, query, pageParam),
    initialPageParam: 0,
    getNextPageParam: (page) => page.nextCursor,
    enabled: Boolean(session && currentPath),
    staleTime: Infinity,
  });
  const assets = useMemo(
    () => assetsQuery.data?.pages.flatMap((page) => page.items) ?? [],
    [assetsQuery.data],
  );
  const total = assetsQuery.data?.pages[0]?.total ?? 0;
  const activeAsset = assets.find((asset) => asset.id === activeId);

  const libraryQuery = useQuery({
    queryKey: ["library-roots"],
    queryFn: listLibraryRoots,
  });

  const handleOpen = useCallback(async () => {
    setError(undefined);
    try {
      const path = await chooseFolder();
      if (!path) return;
      const opened = await openFolder(path);
      clearSelection();
      setSession(opened);
      setCurrentPath(opened.rootPath);
    } catch (cause) {
      setError(String(cause));
    }
  }, [clearSelection]);

  const handleNavigate = useCallback((path: string) => {
    clearSelection();
    setCurrentPath(path);
  }, [clearSelection]);

  const handleRefresh = useCallback(async () => {
    if (!session || !currentPath || isRefreshing) return;
    setError(undefined);
    setIsRefreshing(true);
    try {
      await Promise.all([
        queryClient.cancelQueries({ queryKey: ["assets", session.id, currentPath] }),
        queryClient.cancelQueries({ queryKey: ["directories", session.id, currentPath] }),
      ]);
      await refreshDirectory(session.id, currentPath);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["assets", session.id, currentPath] }),
        queryClient.invalidateQueries({ queryKey: ["directories", session.id, currentPath] }),
      ]);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setIsRefreshing(false);
    }
  }, [currentPath, isRefreshing, queryClient, session]);

  const handleAddLibrary = useCallback(async () => {
    if (!session) return;
    try {
      await addLibraryRoot(session.rootPath);
      await queryClient.invalidateQueries({ queryKey: ["library-roots"] });
    } catch (cause) {
      setError(String(cause));
    }
  }, [queryClient, session]);

  if (!session) {
    return (
      <div className="app-shell app-shell--empty">
        <EmptyState onOpen={handleOpen} t={t} />
        <div className="window-brand"><Aperture size={15} /> OXYVIEWER</div>
        {!isTauri() ? <span className="demo-pill">{t("demoHint")}</span> : null}
      </div>
    );
  }

  return (
    <div
      className={`app-shell ${leftPanelOpen ? "" : "sidebar-collapsed"} ${inspectorOpen ? "" : "inspector-collapsed"}`}
    >
      {leftPanelOpen ? (
        <Sidebar
          session={session}
          currentPath={currentPath ?? session.rootPath}
          libraryRoots={libraryQuery.data ?? []}
          onOpen={handleOpen}
          onNavigate={handleNavigate}
          onRefresh={handleRefresh}
          isRefreshing={isRefreshing}
          onAddLibrary={handleAddLibrary}
          t={t}
        />
      ) : null}
      <section className="workspace">
        <Toolbar total={total} t={t} />
        {assetsQuery.isLoading ? (
          <div className="workspace-loading"><Aperture size={24} /> Scanning {session.displayName}…</div>
        ) : assetsQuery.isError ? (
          <div className="workspace-error"><CircleAlert size={24} /><strong>{String(assetsQuery.error)}</strong><button onClick={handleOpen}><FolderOpen size={15} />{t("openFolder")}</button></div>
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
            currentPath?.split(/[\\/]/).filter(Boolean).at(-1) ?? session.displayName
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
      {inspectorOpen ? <Inspector asset={activeAsset} selectedCount={selectedIds.length} t={t} /> : null}
      {error ? <button className="error-toast" onClick={() => setError(undefined)}><CircleAlert size={16} />{error}<span>×</span></button> : null}
    </div>
  );
}
