import { useQueries, useQuery } from "@tanstack/react-query";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Ellipsis,
  Folder,
  FolderOpen,
  GripVertical,
  LoaderCircle,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Trash2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { listDirectories, searchDirectories } from "../lib/api";
import { buildDirectorySearchTree, type DirectorySearchTreeNode } from "../lib/directorySearchTree";
import {
  moveFolderRelative,
  type FolderDropPlacement,
  type FolderSort,
} from "../lib/folderOrdering";
import type { MessageKey } from "../lib/i18n";
import { platformFileManager } from "../lib/folderPaths";
import type { DirectorySummary, FolderSession } from "../types";
import { ConfirmTrashDialog } from "./ConfirmTrashDialog";

interface SidebarProps {
  sessions: FolderSession[];
  activeSession?: FolderSession;
  currentPath?: string;
  showOnboarding: boolean;
  onOpen: () => void;
  onNavigate: (session: FolderSession, path: string) => void;
  onRemove: (session: FolderSession) => void;
  onTrashFolder: (session: FolderSession, path: string) => void;
  onCopyFolderPath: (session: FolderSession, path: string, relative: boolean) => void;
  onOpenInFileManager: (path: string) => void;
  onRefresh: () => void;
  isRefreshing: boolean;
  onDismissOnboarding: () => void;
  onSettings: () => void;
  folderSort: FolderSort;
  onFolderSortChange: (sort: FolderSort) => void;
  folderDragEnabled: boolean;
  onFolderDragEnabledChange: (enabled: boolean) => void;
  onReorderFolders: (rootPaths: string[]) => void;
  t: (key: MessageKey) => string;
}

const FOLDER_SORT_OPTIONS = [
  ["import", "folderSortImport"],
  ["nameAscending", "folderSortNameAscending"],
  ["nameDescending", "folderSortNameDescending"],
] as const satisfies ReadonlyArray<readonly [FolderSort, MessageKey]>;

const FILE_MANAGER_LABEL = {
  finder: "openInFinder",
  windowsExplorer: "openInWindowsExplorer",
  generic: "openInFileManager",
} as const satisfies Record<ReturnType<typeof platformFileManager>, MessageKey>;

interface DirectoryNodeProps {
  session: FolderSession;
  entry: DirectorySummary;
  currentPath?: string;
  depth: number;
  initiallyExpanded?: boolean;
  onNavigate: (session: FolderSession, path: string) => void;
  onContextMenu: (event: React.MouseEvent, session: FolderSession, entry: DirectorySummary) => void;
  onRemove?: (session: FolderSession) => void;
  removeLabel: string;
  dragLabel?: string;
  rootDraggable?: boolean;
  onRootPointerDown?: React.PointerEventHandler<HTMLButtonElement>;
}

function DirectoryNode({
  session,
  entry,
  currentPath,
  depth,
  initiallyExpanded = false,
  onNavigate,
  onContextMenu,
  onRemove,
  removeLabel,
  dragLabel,
  rootDraggable = false,
  onRootPointerDown,
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
        onContextMenu={(event) => onContextMenu(event, session, entry)}
      >
        {rootDraggable ? (
          <button
            className="tree-row__drag-handle"
            type="button"
            title={dragLabel}
            aria-label={dragLabel}
            onPointerDown={onRootPointerDown}
          >
            <GripVertical size={13} />
          </button>
        ) : null}
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
              onContextMenu={onContextMenu}
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
  onContextMenu,
}: {
  session: FolderSession;
  node: DirectorySearchTreeNode;
  currentPath?: string;
  depth: number;
  search: string;
  onNavigate: (session: FolderSession, path: string) => void;
  onContextMenu: (event: React.MouseEvent, session: FolderSession, entry: DirectorySummary) => void;
}) {
  const [expanded, setExpanded] = useState(true);
  const hasChildren = node.children.length > 0;
  const isActive = currentPath === node.entry.path;

  return (
    <div className="directory-node">
      <div
        className={`tree-row tree-row--directory tree-row--search ${isActive ? "tree-row--active" : ""} ${node.matched ? "tree-row--match" : ""}`}
        style={{ "--tree-indent": `${depth * 13}px` } as React.CSSProperties}
        onContextMenu={(event) => onContextMenu(event, session, node.entry)}
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
              onContextMenu={onContextMenu}
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
  onTrashFolder,
  onCopyFolderPath,
  onOpenInFileManager,
  onRefresh,
  isRefreshing,
  onDismissOnboarding,
  onSettings,
  folderSort,
  onFolderSortChange,
  folderDragEnabled,
  onFolderDragEnabledChange,
  onReorderFolders,
  t,
}: SidebarProps) {
  const [searchOpen, setSearchOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);
  const sortMenuRef = useRef<HTMLDivElement>(null);
  const sortMenuButtonRef = useRef<HTMLButtonElement>(null);
  const sortPopoverRef = useRef<HTMLDivElement>(null);
  const sortSubmenuButtonRef = useRef<HTMLButtonElement>(null);
  const [sortMenuOpen, setSortMenuOpen] = useState(false);
  const [sortSubmenuOpen, setSortSubmenuOpen] = useState(false);
  const [sortMenuPosition, setSortMenuPosition] = useState({ top: 0, left: 0 });
  const [contextMenu, setContextMenu] = useState<{
    session: FolderSession;
    entry: DirectorySummary;
    x: number;
    y: number;
  }>();
  const [pendingTrash, setPendingTrash] = useState<{
    session: FolderSession;
    entry: DirectorySummary;
  }>();
  const [draggedRoot, setDraggedRoot] = useState<string>();
  const [dropTargetRoot, setDropTargetRoot] = useState<string>();
  const [dropPlacement, setDropPlacement] = useState<FolderDropPlacement>();
  const [dragOrder, setDragOrder] = useState<string[]>();
  const [dragPreview, setDragPreview] = useState<{
    left: number;
    top: number;
    width: number;
    height: number;
    pointerOffsetY: number;
    name: string;
  }>();
  const draggedRootRef = useRef<string | undefined>(undefined);
  const dragOrderRef = useRef<string[] | undefined>(undefined);
  const dragPointerIdRef = useRef<number | undefined>(undefined);
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

  useEffect(() => {
    if (!sortMenuOpen) return;
    sortPopoverRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    const closeOnOutsidePress = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!sortMenuRef.current?.contains(target) && !sortPopoverRef.current?.contains(target)) {
        setSortMenuOpen(false);
        setSortSubmenuOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setSortMenuOpen(false);
        setSortSubmenuOpen(false);
        sortMenuButtonRef.current?.focus();
      }
    };
    document.addEventListener("pointerdown", closeOnOutsidePress);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePress);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [sortMenuOpen]);

  const showFolderContextMenu = useCallback((
    event: React.MouseEvent,
    session: FolderSession,
    entry: DirectorySummary,
  ) => {
    event.preventDefault();
    event.stopPropagation();
    setSortMenuOpen(false);
    setSortSubmenuOpen(false);
    setContextMenu({
      session,
      entry,
      x: Math.max(8, Math.min(event.clientX, window.innerWidth - 224)),
      y: Math.max(8, Math.min(event.clientY, window.innerHeight - 132)),
    });
  }, []);

  useEffect(() => {
    if (!contextMenu) return;
    const dismiss = () => setContextMenu(undefined);
    const dismissOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") dismiss();
    };
    window.addEventListener("pointerdown", dismiss);
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    window.addEventListener("keydown", dismissOnEscape);
    document.addEventListener("scroll", dismiss, true);
    return () => {
      window.removeEventListener("pointerdown", dismiss);
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("keydown", dismissOnEscape);
      document.removeEventListener("scroll", dismiss, true);
    };
  }, [contextMenu]);

  const closeSearch = () => {
    setSearchOpen(false);
    setSearch("");
    setDebouncedSearch("");
  };
  const waitingForDebounce = searchActive && search.trim() !== normalizedSearch;
  const searchLoading = waitingForDebounce || searchQueries.some((query) => query.isLoading);
  const indexing = normalizedSearch.length > 0 && searchQueries.some((query) => query.data === null);
  const resultCount = searchQueries.reduce((total, query) => total + (query.data?.length ?? 0), 0);
  const displayedSessions = useMemo(() => {
    if (!dragOrder) return sessions;
    const sessionsByPath = new Map(sessions.map((session) => [session.rootPath, session]));
    return dragOrder.flatMap((path) => {
      const session = sessionsByPath.get(path);
      return session ? [session] : [];
    });
  }, [dragOrder, sessions]);

  const clearFolderDrag = useCallback(() => {
    draggedRootRef.current = undefined;
    dragOrderRef.current = undefined;
    dragPointerIdRef.current = undefined;
    setDraggedRoot(undefined);
    setDropTargetRoot(undefined);
    setDropPlacement(undefined);
    setDragOrder(undefined);
    setDragPreview(undefined);
  }, []);

  useEffect(() => {
    if (!draggedRoot) return;
    document.body.classList.add("is-folder-dragging");

    const moveFolder = (event: PointerEvent) => {
      if (event.pointerId !== dragPointerIdRef.current) return;
      event.preventDefault();
      setDragPreview((preview) => preview
        ? { ...preview, top: event.clientY - preview.pointerOffsetY }
        : preview);

      const target = document
        .elementFromPoint(event.clientX, event.clientY)
        ?.closest<HTMLElement>(".folder-root");
      const targetPath = target?.dataset.rootPath;
      if (!targetPath || targetPath === draggedRoot) {
        setDropTargetRoot(undefined);
        setDropPlacement(undefined);
        return;
      }

      const row = target.querySelector<HTMLElement>(":scope > .directory-node > .tree-row");
      if (!row) return;
      const bounds = row.getBoundingClientRect();
      const placement = event.clientY < bounds.top + bounds.height / 2 ? "before" : "after";
      const currentOrder = dragOrderRef.current ?? sessions.map((item) => item.rootPath);
      const nextOrder = moveFolderRelative(currentOrder, draggedRoot, targetPath, placement);
      if (nextOrder !== currentOrder) {
        dragOrderRef.current = nextOrder;
        setDragOrder(nextOrder);
      }
      setDropTargetRoot(targetPath);
      setDropPlacement(placement);
    };

    const finishFolderDrag = (event: PointerEvent) => {
      if (event.pointerId !== dragPointerIdRef.current) return;
      const reordered = dragOrderRef.current;
      if (reordered?.some((path, index) => path !== sessions[index]?.rootPath)) {
        onReorderFolders(reordered);
      }
      clearFolderDrag();
    };

    window.addEventListener("pointermove", moveFolder, { passive: false });
    window.addEventListener("pointerup", finishFolderDrag);
    window.addEventListener("pointercancel", finishFolderDrag);
    return () => {
      document.body.classList.remove("is-folder-dragging");
      window.removeEventListener("pointermove", moveFolder);
      window.removeEventListener("pointerup", finishFolderDrag);
      window.removeEventListener("pointercancel", finishFolderDrag);
    };
  }, [clearFolderDrag, draggedRoot, onReorderFolders, sessions]);

  return (
    <aside className="sidebar">
      <div className="sidebar__brand">
        <div><strong>OxyViewer</strong><small>PHOTO DESK</small></div>
      </div>

      <div className={`sidebar__section sidebar__section--folders ${showOnboarding ? "is-guided" : ""}`}>
        <div className="sidebar__heading">
          <span>{t("folders")}</span>
          <div className="sidebar__heading-actions">
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
            <div className="folder-sort-control" ref={sortMenuRef}>
              <button
                ref={sortMenuButtonRef}
                className="folder-sort-control__trigger"
                title={t("folderActions")}
                aria-label={t("folderActions")}
                aria-haspopup="menu"
                aria-expanded={sortMenuOpen}
                disabled={sessions.length === 0}
                onClick={(event) => {
                  const rect = event.currentTarget.getBoundingClientRect();
                  const sidebarRight = event.currentTarget
                    .closest(".sidebar")
                    ?.getBoundingClientRect().right;
                  setSortMenuPosition({
                    top: rect.bottom + 4,
                    left: (sidebarRight ?? rect.right) + 2,
                  });
                  setSortSubmenuOpen(false);
                  setSortMenuOpen((open) => !open);
                }}
              >
                <Ellipsis size={14} />
              </button>
            </div>
          </div>
        </div>

        {sortMenuOpen ? createPortal(
          <div
            ref={sortPopoverRef}
            className="folder-command-menu"
            role="menu"
            aria-label={t("folderActions")}
            style={sortMenuPosition}
          >
            <button
              className="folder-command-menu__submenu-trigger"
              ref={sortSubmenuButtonRef}
              role="menuitem"
              aria-haspopup="menu"
              aria-expanded={sortSubmenuOpen}
              onMouseEnter={() => setSortSubmenuOpen(true)}
              onClick={() => setSortSubmenuOpen((open) => !open)}
              onKeyDown={(event) => {
                if (event.key === "ArrowRight") {
                  setSortSubmenuOpen(true);
                  requestAnimationFrame(() => {
                    sortPopoverRef.current
                      ?.querySelector<HTMLButtonElement>('[role="menuitemradio"]')
                      ?.focus();
                  });
                }
              }}
            >
              <span className="folder-command-menu__label">{t("folderSort")}</span>
              <ChevronRight size={13} />
            </button>
            <div className="folder-command-menu__separator" />
            <button
              className="folder-command-menu__checkable"
              role="menuitemcheckbox"
              aria-checked={folderDragEnabled}
              onMouseEnter={() => setSortSubmenuOpen(false)}
              onClick={() => {
                onFolderDragEnabledChange(!folderDragEnabled);
                setSortMenuOpen(false);
                setSortSubmenuOpen(false);
                sortMenuButtonRef.current?.focus();
              }}
            >
              <span className="folder-command-menu__check">
                {folderDragEnabled ? <Check size={13} /> : null}
              </span>
              <span className="folder-command-menu__label">{t("enableFolderDrag")}</span>
              <span />
            </button>
            {sortSubmenuOpen ? (
              <div
                className="folder-command-menu folder-command-submenu"
                role="menu"
                aria-label={t("folderSort")}
                onKeyDown={(event) => {
                  if (event.key === "ArrowLeft") {
                    setSortSubmenuOpen(false);
                    sortSubmenuButtonRef.current?.focus();
                  }
                }}
              >
                {FOLDER_SORT_OPTIONS.map(([value, label]) => (
                  <button
                    key={value}
                    className="folder-command-menu__checkable"
                    role="menuitemradio"
                    aria-checked={folderSort === value}
                    onClick={() => {
                      onFolderSortChange(value);
                      setSortMenuOpen(false);
                      setSortSubmenuOpen(false);
                      sortMenuButtonRef.current?.focus();
                    }}
                  >
                    <span className="folder-command-menu__check">
                      {folderSort === value ? <Check size={13} /> : null}
                    </span>
                    <span className="folder-command-menu__label">{t(label)}</span>
                    <span />
                  </button>
                ))}
              </div>
            ) : null}
          </div>,
          document.body,
        ) : null}

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
                      onNavigate={onNavigate}
                      onContextMenu={showFolderContextMenu}
                    />
                  );
                })}
                {indexing ? <div className="folder-search__state">{t("folderIndexing")}</div> : null}
              </>
            )}
          </div>
        ) : displayedSessions.map((session) => (
          <div
            key={session.rootPath}
            data-root-path={session.rootPath}
            className={`folder-root ${draggedRoot === session.rootPath ? "is-dragging" : ""} ${dropTargetRoot === session.rootPath && dropPlacement ? `is-drop-${dropPlacement}` : ""}`}
          >
            <DirectoryNode
              session={session}
              entry={{ path: session.rootPath, name: session.displayName, hasChildren: true }}
              currentPath={activeSession?.rootPath === session.rootPath ? currentPath : undefined}
              depth={0}
              initiallyExpanded={activeSession?.rootPath === session.rootPath}
              onNavigate={onNavigate}
              onContextMenu={showFolderContextMenu}
              onRemove={onRemove}
              removeLabel={t("removeFolder")}
              dragLabel={t("dragFolderToReorder")}
              rootDraggable={folderDragEnabled && folderSort === "import"}
              onRootPointerDown={(event) => {
                if (event.button !== 0 || !event.isPrimary) return;
                event.preventDefault();
                const initialOrder = sessions.map((item) => item.rootPath);
                const row = event.currentTarget.closest<HTMLElement>(".tree-row");
                if (!row) return;
                const bounds = row.getBoundingClientRect();
                draggedRootRef.current = session.rootPath;
                dragOrderRef.current = initialOrder;
                dragPointerIdRef.current = event.pointerId;
                setDraggedRoot(session.rootPath);
                setDragOrder(initialOrder);
                setDragPreview({
                  left: bounds.left,
                  top: bounds.top,
                  width: bounds.width,
                  height: bounds.height,
                  pointerOffsetY: event.clientY - bounds.top,
                  name: session.displayName,
                });
              }}
            />
          </div>
        ))}

        {dragPreview ? createPortal(
          <div
            className="folder-drag-preview"
            style={{
              left: dragPreview.left,
              top: dragPreview.top,
              width: dragPreview.width,
              height: dragPreview.height,
            }}
            aria-hidden="true"
          >
            <GripVertical size={13} />
            <span className="folder-drag-preview__toggle"><ChevronRight size={13} /></span>
            <Folder size={15} />
            <span>{dragPreview.name}</span>
          </div>,
          document.body,
        ) : null}

        {contextMenu ? createPortal(
          <div
            className="folder-context-menu"
            role="menu"
            aria-label={contextMenu.entry.name}
            style={{ left: contextMenu.x, top: contextMenu.y }}
            onContextMenu={(event) => event.preventDefault()}
            onPointerDown={(event) => event.stopPropagation()}
          >
            <button
              autoFocus
              role="menuitem"
              onClick={() => {
                const { session, entry } = contextMenu;
                setContextMenu(undefined);
                onCopyFolderPath(session, entry.path, true);
              }}
            >
              <Copy size={13} />
              {t("copyRelativePath")}
            </button>
            <button
              role="menuitem"
              onClick={() => {
                const { session, entry } = contextMenu;
                setContextMenu(undefined);
                onCopyFolderPath(session, entry.path, false);
              }}
            >
              <Copy size={13} />
              {t("copyAbsolutePath")}
            </button>
            <button
              role="menuitem"
              onClick={() => {
                const { entry } = contextMenu;
                setContextMenu(undefined);
                onOpenInFileManager(entry.path);
              }}
            >
              <FolderOpen size={13} />
              {t(FILE_MANAGER_LABEL[platformFileManager()])}
            </button>
            <div className="folder-context-menu__separator" />
            <button
              className="folder-context-menu__danger"
              role="menuitem"
              onClick={() => {
                const { session, entry } = contextMenu;
                setContextMenu(undefined);
                setPendingTrash({ session, entry });
              }}
            >
              <Trash2 size={13} />
              {t("trashFolder")}
            </button>
          </div>,
          document.body,
        ) : null}

        {pendingTrash ? (
          <ConfirmTrashDialog
            itemName={pendingTrash.entry.name}
            onCancel={() => setPendingTrash(undefined)}
            onConfirm={() => {
              const { session, entry } = pendingTrash;
              setPendingTrash(undefined);
              onTrashFolder(session, entry.path);
            }}
            t={t}
          />
        ) : null}

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
