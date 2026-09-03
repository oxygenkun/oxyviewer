import { useQueries, useQuery } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  LoaderCircle,
  Plus,
  RefreshCw,
  Search,
  Settings,
  X,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { listDirectories, searchDirectories } from "../lib/api";
import { buildDirectorySearchTree, type DirectorySearchTreeNode } from "../lib/directorySearchTree";
import type { MessageKey } from "../lib/i18n";
import type { DirectorySummary, FolderSession } from "../types";

interface SidebarProps {
  sessions: FolderSession[];
  activeSession?: FolderSession;
  currentPath?: string;
  showOnboarding: boolean;
  onOpen: () => void;
  onNavigate: (session: FolderSession, path: string) => void;
  onRemove: (session: FolderSession) => void;
  onRefresh: () => void;
  isRefreshing: boolean;
  onDismissOnboarding: () => void;
  onSettings: () => void;
  t: (key: MessageKey) => string;
}

interface DirectoryNodeProps {
  session: FolderSession;
  entry: DirectorySummary;
  currentPath?: string;
  depth: number;
  initiallyExpanded?: boolean;
  onNavigate: (session: FolderSession, path: string) => void;
  onRemove?: (session: FolderSession) => void;
  removeLabel: string;
}

function DirectoryNode({
  session,
  entry,
  currentPath,
  depth,
  initiallyExpanded = false,
  onNavigate,
  onRemove,
  removeLabel,
}: DirectoryNodeProps) {
  const [expanded, setExpanded] = useState(initiallyExpanded);
  const children = useQuery({
    queryKey: ["directories", session.id, entry.path],
    queryFn: () => listDirectories(session.id, entry.path),
    enabled: expanded,
    staleTime: Infinity,
  });
  const isActive = currentPath === entry.path;
  const hasChildren = children.data ? children.data.length > 0 : entry.hasChildren;

  return (
    <div className="directory-node">
      <div
        className={`tree-row tree-row--directory ${isActive ? "tree-row--active" : ""}`}
        style={{ "--tree-indent": `${depth * 13}px` } as React.CSSProperties}
      >
        <button
          className="tree-row__toggle"
          disabled={!hasChildren && !children.isLoading}
          onClick={() => setExpanded((open) => !open)}
          aria-label={expanded ? "Collapse folder" : "Expand folder"}
        >
          {children.isLoading ? (
            <LoaderCircle className="tree-row__loader" size={12} />
          ) : hasChildren ? (
            expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />
          ) : (
            <span />
          )}
        </button>
        <button
          className="tree-row__main"
          onClick={() => onNavigate(session, entry.path)}
          title={entry.path}
        >
          {isActive ? <FolderOpen size={15} /> : <Folder size={15} />}
          <span>{entry.name}</span>
          {isActive ? <i /> : null}
        </button>
        {onRemove ? (
          <button
            className="tree-row__remove"
            onClick={() => onRemove(session)}
            title={removeLabel}
            aria-label={`${removeLabel}: ${entry.name}`}
          >
            <X size={12} />
          </button>
        ) : null}
      </div>
      {expanded ? (
        <div className="directory-node__children">
          {(children.data ?? []).map((child) => (
            <DirectoryNode
              key={child.path}
              session={session}
              entry={child}
              currentPath={currentPath}
              depth={depth + 1}
              onNavigate={onNavigate}
              removeLabel={removeLabel}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function SearchDirectoryNode({
  session,
  node,
  currentPath,
  depth,
  search,
  onNavigate,
}: {
  session: FolderSession;
  node: DirectorySearchTreeNode;
  currentPath?: string;
  depth: number;
  search: string;
  onNavigate: (session: FolderSession, path: string) => void;
}) {
  const [expanded, setExpanded] = useState(true);
  const hasChildren = node.children.length > 0;
  const isActive = currentPath === node.entry.path;

  return (
    <div className="directory-node">
      <div
        className={`tree-row tree-row--directory tree-row--search ${isActive ? "tree-row--active" : ""} ${node.matched ? "tree-row--match" : ""}`}
        style={{ "--tree-indent": `${depth * 13}px` } as React.CSSProperties}
      >
        <button
          className="tree-row__toggle"
          disabled={!hasChildren}
          onClick={() => setExpanded((open) => !open)}
          aria-label={expanded ? "Collapse folder" : "Expand folder"}
        >
          {hasChildren ? expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} /> : <span />}
        </button>
        <button
          className="tree-row__main"
          onClick={() => onNavigate(session, node.entry.path)}
          title={node.entry.path}
        >
          {isActive ? <FolderOpen size={15} /> : <Folder size={15} />}
          <HighlightedDirectoryName name={node.entry.name} search={node.matched ? search : ""} />
          {isActive ? <i /> : null}
        </button>
      </div>
      {expanded ? (
        <div className="directory-node__children">
          {node.children.map((child) => (
            <SearchDirectoryNode
              key={child.entry.path}
              session={session}
              node={child}
              currentPath={currentPath}
              depth={depth + 1}
              search={search}
              onNavigate={onNavigate}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function HighlightedDirectoryName({ name, search }: { name: string; search: string }) {
  if (!search) return <span>{name}</span>;
  const start = name.toLocaleLowerCase().indexOf(search.toLocaleLowerCase());
  if (start < 0) return <span>{name}</span>;
  const end = start + search.length;
  return (
    <span>
      {name.slice(0, start)}
      <mark>{name.slice(start, end)}</mark>
      {name.slice(end)}
    </span>
  );
}

export function Sidebar({
  sessions,
  activeSession,
  currentPath,
  showOnboarding,
  onOpen,
  onNavigate,
  onRemove,
  onRefresh,
  isRefreshing,
  onDismissOnboarding,
  onSettings,
  t,
}: SidebarProps) {
  const [searchOpen, setSearchOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);
  const normalizedSearch = debouncedSearch.trim();
  const searchActive = searchOpen && search.trim().length > 0;
  const searchQueries = useQueries({
    queries: sessions.map((session) => ({
      queryKey: ["directory-search", session.id, normalizedSearch],
      queryFn: () => searchDirectories(session.id, normalizedSearch),
      enabled: searchOpen && normalizedSearch.length > 0,
      staleTime: Infinity,
    })),
  });

  useEffect(() => {
    const timeout = window.setTimeout(() => setDebouncedSearch(search), 180);
    return () => window.clearTimeout(timeout);
  }, [search]);

  useEffect(() => {
    if (searchOpen) searchInputRef.current?.focus();
  }, [searchOpen]);

  const closeSearch = () => {
    setSearchOpen(false);
    setSearch("");
    setDebouncedSearch("");
  };
  const navigateFromSearch = (session: FolderSession, path: string) => {
    onNavigate(session, path);
    closeSearch();
  };
  const waitingForDebounce = searchActive && search.trim() !== normalizedSearch;
  const searchLoading = waitingForDebounce || searchQueries.some((query) => query.isLoading);
  const indexing = normalizedSearch.length > 0 && searchQueries.some((query) => query.data === null);
  const resultCount = searchQueries.reduce((total, query) => total + (query.data?.length ?? 0), 0);

  return (
    <aside className="sidebar">
      <div className="sidebar__brand">
        <span className="brand-glyph">OX</span>
        <div><strong>OxyViewer</strong><small>PHOTO DESK</small></div>
      </div>

      <div className={`sidebar__section sidebar__section--folders ${showOnboarding ? "is-guided" : ""}`}>
        <div className="sidebar__heading">
          <span>{t("folders")}</span>
          <span className="sidebar__heading-actions">
            <button
              className={searchOpen ? "is-active" : undefined}
              title={t("searchFolders")}
              aria-label={t("searchFolders")}
              disabled={sessions.length === 0}
              onClick={() => searchOpen ? closeSearch() : setSearchOpen(true)}
            >
              <Search size={13} />
            </button>
            <button
              title={t("refreshFolder")}
              aria-label={t("refreshFolder")}
              disabled={!activeSession || isRefreshing}
              onClick={onRefresh}
            >
              <RefreshCw className={isRefreshing ? "tree-row__loader" : undefined} size={13} />
            </button>
            <button className="sidebar__add-folder" title={t("openFolder")} onClick={onOpen}>
              <Plus size={14} />
            </button>
          </span>
        </div>

        {searchOpen ? (
          <label className="folder-search">
            <Search size={13} />
            <input
              ref={searchInputRef}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") closeSearch();
              }}
              placeholder={t("searchFolderNames")}
              aria-label={t("searchFolderNames")}
            />
            {search ? (
              <button onClick={() => setSearch("")} title={t("clearFolderSearch")}>
                <X size={12} />
              </button>
            ) : null}
          </label>
        ) : null}

        {searchActive ? (
          <div className="folder-search__results" aria-live="polite">
            {searchLoading ? (
              <div className="folder-search__state"><LoaderCircle className="tree-row__loader" size={13} />{t("searchingFolders")}</div>
            ) : indexing && resultCount === 0 ? (
              <div className="folder-search__state">{t("folderIndexing")}</div>
            ) : resultCount === 0 ? (
              <div className="folder-search__state">{t("noMatchingFolders")}</div>
            ) : (
              <>
                {sessions.map((session, index) => {
                  const matches = searchQueries[index]?.data ?? [];
                  if (matches.length === 0) return null;
                  const tree = buildDirectorySearchTree(
                    { path: session.rootPath, name: session.displayName, hasChildren: true },
                    matches,
                  );
                  return (
                    <SearchDirectoryNode
                      key={`${normalizedSearch}:${session.rootPath}`}
                      session={session}
                      node={tree}
                      currentPath={activeSession?.rootPath === session.rootPath ? currentPath : undefined}
                      depth={0}
                      search={normalizedSearch}
                      onNavigate={navigateFromSearch}
                    />
                  );
                })}
                {indexing ? <div className="folder-search__state">{t("folderIndexing")}</div> : null}
              </>
            )}
          </div>
        ) : sessions.map((session) => (
            <DirectoryNode
              key={session.rootPath}
              session={session}
              entry={{ path: session.rootPath, name: session.displayName, hasChildren: true }}
              currentPath={activeSession?.rootPath === session.rootPath ? currentPath : undefined}
              depth={0}
              initiallyExpanded={activeSession?.rootPath === session.rootPath}
              onNavigate={onNavigate}
              onRemove={onRemove}
              removeLabel={t("removeFolder")}
            />
          ))}

        {sessions.length === 0 ? (
          <button className="sidebar__folder-prompt" onClick={onOpen}>
            <Plus size={15} />
            <span>{t("chooseFolderHere")}</span>
          </button>
        ) : null}

        {showOnboarding ? (
          <div className="folder-coach" role="dialog" aria-label={t("firstRunTitle")}>
            <span className="folder-coach__pointer" />
            <strong>{t("firstRunTitle")}</strong>
            <p>{t("firstRunBody")}</p>
            <div>
              <button onClick={onDismissOnboarding}>{t("gotIt")}</button>
              <button className="folder-coach__action" onClick={onOpen}>{t("chooseFolder")}</button>
            </div>
          </div>
        ) : null}
      </div>

      <div className="sidebar__bottom">
        <button
          className="sidebar__bottom-btn"
          title={t("settings")}
          aria-label={t("settings")}
          onClick={onSettings}
        >
          <Settings size={15} />
        </button>
      </div>
      <div className="sidebar__path" title={currentPath}>{currentPath ?? t("noFolderOpen")}</div>
    </aside>
  );
}
