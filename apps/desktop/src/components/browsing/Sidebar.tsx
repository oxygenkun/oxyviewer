import { useQueries, useQueryClient } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  Crosshair,
  Folder,
  FolderOpen,
  GripVertical,
  Images,
  ListCollapse,
  LoaderCircle,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Trash2,
  Users,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { collapseDirectoryTree, getDirectoryTree, searchDirectories, setDirectoryExpanded } from "@/lib/api";
import {
  acceptDirectoryTreeSnapshot,
  directoryTreePlaceholder,
  directoryRevealStep,
} from "@/lib/projection/directoryTreeProjection";
import { buildDirectorySearchTree, type DirectorySearchTreeNode } from "@/lib/browse/directorySearchTree";
import {
  moveFolderRelative,
  type FolderDropPlacement,
  type FolderSort,
} from "@/lib/browse/folderOrdering";
import type { MessageKey } from "@/lib/i18n";
import type { FolderRestoreState } from "@/lib/browse/folderRestoration";
import { isSameOrDescendantPath, platformFileManager } from "@/lib/browse/folderPaths";
import type { DirectorySummary, DirectoryTreeNode, DirectoryTreeSnapshot, FolderPerson, FolderSession, PersonFilterState } from "@/types";
import { ConfirmTrashDialog } from "@/components/overlay/ConfirmTrashDialog";
import { FolderNameButton } from "./FolderNameButton";
import { UnavailableFolder } from "./UnavailableFolder";
import type { RootRelocationPlan } from "@/types";
import { FolderSettingsMenu } from "./FolderSettingsMenu";
import { PersonAnalysisControls } from "../people/PersonAnalysisControls";

interface SidebarProps {
  peopleMode?: boolean;
  onPeopleModeChange?: (value: boolean) => void;
  people?: FolderPerson[];
  selectedPersonId?: string;
  onSelectPerson?: (id: string) => void;
  onCreatePerson?: () => void;
  personFilterState?: PersonFilterState | "off";
  onPersonFilterChange?: (value: PersonFilterState | "off") => void;
  showBoxes?: boolean;
  onShowBoxesChange?: (value: boolean) => void;
  personReview?: ReactNode;
  onRenamePerson?: (id: string, displayName: string) => void;
  sessions: FolderSession[];
  total?: number;
  folderRestoreStates?: FolderRestoreState[];
  onRetryRoot: (path: string) => Promise<void>;
  onRemoveRoot: (path: string) => Promise<void>;
  onRelocateRoot: (plan: RootRelocationPlan) => Promise<void>;
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

const FILE_MANAGER_LABEL = {
  finder: "openInFinder",
  windowsExplorer: "openInWindowsExplorer",
  generic: "openInFileManager",
} as const satisfies Record<ReturnType<typeof platformFileManager>, MessageKey>;

interface DirectoryNodeProps {
  session: FolderSession;
  node: DirectoryTreeNode;
  currentPath?: string;
  depth: number;
  onNavigate: (session: FolderSession, path: string) => void;
  onExpandedChange: (session: FolderSession, path: string, expanded: boolean) => void;
  onContextMenu: (event: React.MouseEvent, session: FolderSession, entry: DirectorySummary) => void;
  onRemove?: (session: FolderSession) => void;
  removeLabel: string;
  dragLabel?: string;
  rootDraggable?: boolean;
  onRootPointerDown?: React.PointerEventHandler<HTMLButtonElement>;
}

function DirectoryNode({
  session,
  node,
  currentPath,
  depth,
  onNavigate,
  onExpandedChange,
  onContextMenu,
  onRemove,
  removeLabel,
  dragLabel,
  rootDraggable = false,
  onRootPointerDown,
}: DirectoryNodeProps) {
  const { entry, expanded, children } = node;
  const isActive = currentPath === entry.path;
  const isAncestor = !isActive && !!currentPath && isSameOrDescendantPath(entry.path, currentPath);
  const loading = expanded && children === null;

  return (
    <div className="directory-node">
      <div
        className={`tree-row tree-row--directory ${isActive ? "tree-row--active" : ""} ${isAncestor ? "tree-row--ancestor" : ""}`}
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
          disabled={!entry.hasChildren && !loading}
          onClick={() => onExpandedChange(session, entry.path, !expanded)}
          aria-label={entry.hasChildren || loading
            ? expanded ? "Collapse folder" : "Expand folder"
            : undefined}
        >
          {loading ? (
            <LoaderCircle className="tree-row__loader" size={12} />
          ) : entry.hasChildren ? (
            expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />
          ) : (
            <span />
          )}
        </button>
        <FolderNameButton
          className="tree-row__main"
          onClick={() => {
            onNavigate(session, entry.path);
            if (entry.hasChildren && !expanded) onExpandedChange(session, entry.path, true);
          }}
          title={entry.path}
        >
          {isActive ? <FolderOpen size={15} /> : <Folder size={15} />}
          <span>{entry.name}</span>
          {isActive ? <i /> : null}
        </FolderNameButton>
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
          {(children ?? []).map((child) => (
            <DirectoryNode
              key={child.entry.path}
              session={session}
              node={child}
              currentPath={currentPath}
              depth={depth + 1}
              onNavigate={onNavigate}
              onExpandedChange={onExpandedChange}
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
        <FolderNameButton
          className="tree-row__main"
          onClick={() => onNavigate(session, node.entry.path)}
          title={node.entry.path}
        >
          {isActive ? <FolderOpen size={15} /> : <Folder size={15} />}
          <HighlightedDirectoryName name={node.entry.name} search={node.matched ? search : ""} />
          {isActive ? <i /> : null}
        </FolderNameButton>
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
  peopleMode = false,
  onPeopleModeChange,
  people = [],
  selectedPersonId,
  onSelectPerson,
  onCreatePerson,
  personFilterState = "off",
  onPersonFilterChange,
  showBoxes = true,
  onShowBoxesChange,
  personReview,
  onRenamePerson,
  sessions,
  total = 0,
  folderRestoreStates = [],
  onRetryRoot, onRemoveRoot, onRelocateRoot,
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
  const queryClient = useQueryClient();
  const [searchOpen, setSearchOpen] = useState(false);
  const [peopleFoldersOpen, setPeopleFoldersOpen] = useState(false);
  const [personFolderShare, setPersonFolderShare] = useState(() => {
    const saved = Number(window.localStorage.getItem("oxyviewer.personFolderShare"));
    return saved >= 15 && saved <= 75 ? saved : 35;
  });
  const sidebarRef = useRef<HTMLElement>(null);
  const folderResizeRef = useRef<{ pointerId: number; startY: number; startShare: number } | null>(null);
  const [personFilterOpen, setPersonFilterOpen] = useState(false);
  const [renamingId, setRenamingId] = useState<string>();
  const [renameValue, setRenameValue] = useState("");
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);
  const folderTreeRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const container = folderTreeRef.current;
    if (!container) return;
    let idleTimer: number | undefined;
    const onScroll = () => {
      container.classList.add("is-scrolling");
      window.clearTimeout(idleTimer);
      idleTimer = window.setTimeout(() => container.classList.remove("is-scrolling"), 800);
    };
    container.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      container.removeEventListener("scroll", onScroll);
      window.clearTimeout(idleTimer);
      container.classList.remove("is-scrolling");
    };
  }, []);
  const [sortMenuOpen, setSortMenuOpen] = useState(false);
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
  const directoryTreeQueries = useQueries({
    queries: sessions.map((session) => ({
      queryKey: ["directory-tree", session.id],
      queryFn: async () => {
        const incoming = await getDirectoryTree(session);
        const current = queryClient.getQueryData<DirectoryTreeSnapshot>([
          "directory-tree",
          session.id,
        ]);
        return acceptDirectoryTreeSnapshot(current, incoming);
      },
      placeholderData: directoryTreePlaceholder(session),
      staleTime: Infinity,
    })),
  });
  const initializedTreeSessionsRef = useRef(new Set<string>());
  const syncDirectoryTree = useCallback((snapshot: DirectoryTreeSnapshot) => {
    queryClient.setQueryData<DirectoryTreeSnapshot>(
      ["directory-tree", snapshot.sessionId],
      (current) => acceptDirectoryTreeSnapshot(current, snapshot),
    );
  }, [queryClient]);
  const changeDirectoryExpansion = useCallback((
    session: FolderSession,
    path: string,
    expanded: boolean,
  ) => {
    void queryClient.cancelQueries({ queryKey: ["directory-tree", session.id] })
      .then(() => setDirectoryExpanded(session.id, path, expanded))
      .then(syncDirectoryTree)
      .catch(() => queryClient.invalidateQueries({ queryKey: ["directory-tree", session.id] }));
  }, [queryClient, syncDirectoryTree]);

  const activeTreeQueryIndex = activeSession
    ? sessions.findIndex((session) => session.id === activeSession.id)
    : -1;
  const activeTreeQuery = directoryTreeQueries[activeTreeQueryIndex];
  const wasSearchActiveRef = useRef(false);
  const revealSelectionRef = useRef<{
    sessionId: string;
    path: string;
    requested: Set<string>;
  } | undefined>(undefined);
  const [revealNonce, setRevealNonce] = useState(0);
  useEffect(() => {
    if (wasSearchActiveRef.current && !searchActive && activeSession && currentPath) {
      revealSelectionRef.current = {
        sessionId: activeSession.id,
        path: currentPath,
        requested: new Set(),
      };
    }
    wasSearchActiveRef.current = searchActive;
    const reveal = revealSelectionRef.current;
    if (!reveal) return;
    if (searchActive || reveal.sessionId !== activeSession?.id || reveal.path !== currentPath) {
      revealSelectionRef.current = undefined;
      return;
    }
    if (!activeTreeQuery?.isFetched || !activeTreeQuery.data) return;
    const step = directoryRevealStep(activeTreeQuery.data.root, reveal.path);
    if (step === "done") {
      revealSelectionRef.current = undefined;
      const container = folderTreeRef.current;
      const selectedRow = container?.querySelector<HTMLElement>(".folder-root .tree-row--active");
      if (container && selectedRow) {
        const viewport = container.getBoundingClientRect();
        const row = selectedRow.getBoundingClientRect();
        const top = viewport.top + container.clientTop;
        const bottom = top + container.clientHeight;
        if (row.top < top) {
          container.scrollTop += row.top - top;
        } else if (row.bottom > bottom) {
          container.scrollTop += row.bottom - bottom;
        }
      }
    } else if (step !== "waiting" && !reveal.requested.has(step.expand)) {
      reveal.requested.add(step.expand);
      changeDirectoryExpansion(activeSession, step.expand, true);
    }
  }, [searchActive, activeSession, currentPath, activeTreeQuery?.isFetched,
    activeTreeQuery?.data, changeDirectoryExpansion, revealNonce]);

  useEffect(() => {
    if (!activeSession || !activeTreeQuery?.isFetched) return;
    if (initializedTreeSessionsRef.current.has(activeSession.id)) return;
    initializedTreeSessionsRef.current.add(activeSession.id);
    if (!activeTreeQuery.data?.root.expanded) {
      changeDirectoryExpansion(activeSession, activeSession.rootPath, true);
    }
  }, [
    activeSession,
    activeTreeQuery?.data?.root.expanded,
    activeTreeQuery?.isFetched,
    changeDirectoryExpansion,
  ]);

  useEffect(() => {
    const timeout = window.setTimeout(() => setDebouncedSearch(search), 180);
    return () => window.clearTimeout(timeout);
  }, [search]);

  useEffect(() => {
    if (searchOpen) searchInputRef.current?.focus();
  }, [searchOpen]);

  const showFolderContextMenu = useCallback((
    event: React.MouseEvent,
    session: FolderSession,
    entry: DirectorySummary,
  ) => {
    event.preventDefault();
    event.stopPropagation();
    setSortMenuOpen(false);
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
  const revealCurrentFolder = () => {
    if (!activeSession || !currentPath) return;
    if (searchOpen) closeSearch();
    revealSelectionRef.current = {
      sessionId: activeSession.id,
      path: currentPath,
      requested: new Set(),
    };
    setRevealNonce((nonce) => nonce + 1);
  };
  const collapseAllFolders = () => {
    revealSelectionRef.current = undefined;
    for (const session of sessions) {
      void queryClient.cancelQueries({ queryKey: ["directory-tree", session.id] })
        .then(() => collapseDirectoryTree(session.id))
        .then(syncDirectoryTree)
        .catch(() => queryClient.invalidateQueries({ queryKey: ["directory-tree", session.id] }));
    }
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

  const unavailable = folderRestoreStates.filter(state => state.status !== "ready");
  const folderRows: (FolderSession | FolderRestoreState)[] = displayedSessions.filter(session =>
    !unavailable.some(state => state.rootPath === session.rootPath));
  for (const state of unavailable) {
    const rank = folderRestoreStates.findIndex(item => item.rootPath === state.rootPath);
    const next = folderRows.findIndex(item => {
      const position = folderRestoreStates.findIndex(candidate => candidate.rootPath === item.rootPath);
      return position < 0 || position > rank;
    });
    folderRows.splice(next < 0 ? folderRows.length : next, 0, state);
  }


  return (
    <aside ref={sidebarRef} className={`sidebar ${peopleMode ? "sidebar--people" : ""}`} style={{ "--person-folder-share": `${personFolderShare}%` } as React.CSSProperties}>
      <div className="sidebar__brand">
        <div><strong>OxyViewer</strong><small>PHOTO DESK</small></div>
      </div>

      <div className="person-sidebar-tabs" role="tablist" aria-label="Sidebar view">
        <button role="tab" aria-selected={!peopleMode} onClick={() => onPeopleModeChange?.(false)}>
          <Images size={15} />
          <span>浏览</span>
          <span className="person-sidebar-tabs__count">{total.toLocaleString()}</span>
        </button>
        <button role="tab" aria-selected={peopleMode} onClick={() => onPeopleModeChange?.(true)}>
          <Users size={15} />
          <span>人物</span>
          <span className="person-sidebar-tabs__count">{people.length.toLocaleString()}</span>
        </button>
      </div>
      {peopleMode ? <section className={`person-sidebar-section ${personFilterOpen ? "is-filter-open" : ""}`}>
        <PersonAnalysisControls sessionId={activeSession?.id} folderPath={currentPath} />
        <div className="person-sidebar-heading"><div><span>人物</span><strong>当前文件夹</strong></div><button onClick={onCreatePerson} disabled={!currentPath} aria-label="添加人物">＋ 添加</button></div>
        {!people.length ? <p className="person-sidebar-empty">还没有人物。添加一位人物，然后在照片上标记并审阅。</p> : people.map((person, index) => {
          const selected = selectedPersonId === person.id;
          return (
            <div key={person.id} className={`person-sidebar-item ${selected ? "is-selected" : ""} ${selected && personFilterOpen ? "is-expanded" : ""}`}>
              {renamingId === person.id ? (
                <form
                  className="person-rename"
                  onSubmit={(event) => {
                    event.preventDefault();
                    const next = renameValue.trim();
                    if (next && next !== (person.displayName ?? "")) onRenamePerson?.(person.id, next);
                    setRenamingId(undefined);
                  }}
                >
                  <input
                    autoFocus
                    aria-label="人物名称"
                    value={renameValue}
                    onChange={(event) => setRenameValue(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Escape") setRenamingId(undefined);
                    }}
                  />
                  <button type="submit" disabled={!renameValue.trim()}>保存</button>
                </form>
              ) : (
                <div className="person-sidebar-row">
                  <button
                    aria-expanded={selected ? personFilterOpen : undefined}
                    onClick={() => {
                      if (selected) {
                        setPersonFilterOpen((open) => !open);
                      } else {
                        setPersonFilterOpen(false);
                        onSelectPerson?.(person.id);
                      }
                    }}
                  >
                    <span className="person-sidebar-avatar" aria-hidden="true">{(person.displayName ?? `人物 ${index + 1}`).slice(0, 1)}</span>
                    <span className="person-sidebar-details"><strong>{person.displayName ?? `匿名人物 ${index + 1}`}</strong><small>{person.identityConfirmed ? "身份已确认" : "身份待确认"}</small></span>
                    {(person.pendingCount ?? 0) > 0 ? <span className="person-sidebar-pending" title={`${person.pendingCount} 个实例待审`}>{person.pendingCount}</span> : null}
                    {selected ? personFilterOpen ? <ChevronDown className="person-sidebar-disclosure" size={13} /> : <ChevronRight className="person-sidebar-disclosure" size={13} /> : null}
                  </button>
                  <button
                    className="person-sidebar-rename"
                    title="改名"
                    aria-label={`改名：${person.displayName ?? `匿名人物 ${index + 1}`}`}
                    onClick={() => {
                      setRenamingId(person.id);
                      setRenameValue(person.displayName ?? "");
                    }}
                  >
                    <Pencil size={12} />
                  </button>
                </div>
              )}
              {selected && personFilterOpen && onPersonFilterChange ? (
                <div className="person-filter-panel">
                  <span className="person-filter-panel__label">显示照片</span>
                  <select
                    aria-label="人物审阅过滤"
                    value={personFilterState}
                    onChange={(event) => onPersonFilterChange(event.target.value as PersonFilterState | "off")}
                  >
                    <option value="off">文件夹全部照片 · 手工添加</option>
                    <option value="pending">待确认</option>
                    <option value="belongs">已确认属于</option>
                    <option value="all">全部候选</option>
                    <option value="doesNotBelong">不属于</option>
                    <option value="deferred">暂缓</option>
                    <option value="unassigned">未加入此人物</option>
                    <option value="needsReview">源图变化 · 待复核</option>
                  </select>
                  {personFilterState !== "off" ? (
                    <button className="person-filter-panel__clear" onClick={() => onPersonFilterChange("off")}>清除人物过滤</button>
                  ) : null}
                  <label>
                    <input
                      type="checkbox"
                      checked={showBoxes}
                      onChange={(event) => onShowBoxesChange?.(event.target.checked)}
                    />
                    显示人物框
                  </label>
                  <span>与当前搜索、评级、标签取交集</span>
                </div>
              ) : null}
            </div>
          );
        })}
      </section> : null}

      {personReview}

      {peopleMode && peopleFoldersOpen ? <div
        className="person-folder-resizer"
        role="separator"
        tabIndex={0}
        aria-label="调整人物与文件夹区域高度"
        aria-orientation="horizontal"
        aria-valuemin={15}
        aria-valuemax={75}
        aria-valuenow={Math.round(personFolderShare)}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          folderResizeRef.current = { pointerId: event.pointerId, startY: event.clientY, startShare: personFolderShare };
          event.currentTarget.setPointerCapture(event.pointerId);
          event.preventDefault();
        }}
        onPointerMove={(event) => {
          const drag = folderResizeRef.current;
          const height = sidebarRef.current?.clientHeight;
          if (!drag || drag.pointerId !== event.pointerId || !height) return;
          setPersonFolderShare(Math.round(Math.max(15, Math.min(75, drag.startShare + (drag.startY - event.clientY) * 100 / height))));
        }}
        onPointerUp={(event) => {
          if (folderResizeRef.current?.pointerId !== event.pointerId) return;
          folderResizeRef.current = null;
          window.localStorage.setItem("oxyviewer.personFolderShare", String(personFolderShare));
        }}
        onPointerCancel={() => { folderResizeRef.current = null; }}
        onLostPointerCapture={() => { folderResizeRef.current = null; }}
        onKeyDown={(event) => {
          const delta = event.key === "ArrowUp" ? 2 : event.key === "ArrowDown" ? -2 : 0;
          if (!delta) return;
          event.preventDefault();
          setPersonFolderShare((share) => {
            const next = Math.max(15, Math.min(75, share + delta));
            window.localStorage.setItem("oxyviewer.personFolderShare", String(next));
            return next;
          });
        }}
      /> : null}

      <div className={`sidebar__section sidebar__folders ${showOnboarding ? "is-guided" : ""} ${peopleMode && !peopleFoldersOpen ? "person-folders-collapsed" : ""}`}>
        <div className="sidebar__heading">
          {peopleMode ? <button className="person-folder-toggle" onClick={() => setPeopleFoldersOpen((value) => !value)}>{peopleFoldersOpen ? "▾" : "▸"} {t("folders")} · {currentPath?.split(/[\\/]/).at(-1) ?? "—"}</button> : <span>{t("folders")}</span>}
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
              title={t("revealCurrentFolder")}
              aria-label={t("revealCurrentFolder")}
              disabled={!activeSession || !currentPath}
              onClick={revealCurrentFolder}
            >
              <Crosshair size={13} />
            </button>
            <button
              title={t("collapseAllFolders")}
              aria-label={t("collapseAllFolders")}
              disabled={sessions.length === 0}
              onClick={collapseAllFolders}
            >
              <ListCollapse size={13} />
            </button>
            <button
              title={t("refreshFolder")}
              aria-label={t("refreshFolder")}
              disabled={isRefreshing || (sessions.length === 0 && folderRestoreStates.length === 0)}
              onClick={onRefresh}
            >
              <RefreshCw className={isRefreshing ? "tree-row__loader" : undefined} size={13} />
            </button>
            <button className="sidebar__add-folder" title={t("openFolder")} onClick={onOpen}>
              <Plus size={14} />
            </button>
            <FolderSettingsMenu open={sortMenuOpen} onOpenChange={setSortMenuOpen}
              disabled={sessions.length === 0} folderSort={folderSort}
              onFolderSortChange={onFolderSortChange} folderDragEnabled={folderDragEnabled}
              onFolderDragEnabledChange={onFolderDragEnabledChange} t={t} />
          </div>
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

        <div ref={folderTreeRef} className="sidebar__section--folders">
          {searchActive ? folderRestoreStates.filter(state => state.status !== "ready").map(state =>
            <UnavailableFolder key={state.rootPath} path={state.rootPath} error={state.error} restoring={state.status === "restoring" || state.checking}
              onRetry={onRetryRoot} onRemove={onRemoveRoot} onRelocate={onRelocateRoot} t={t} />) : null}
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
          ) : folderRows.map((session) => {
            if ("status" in session) return <UnavailableFolder key={session.rootPath} path={session.rootPath} error={session.error} restoring={session.status === "restoring" || session.checking}
              onRetry={onRetryRoot} onRemove={onRemoveRoot} onRelocate={onRelocateRoot} t={t} />;
            const sessionIndex = sessions.findIndex((item) => item.id === session.id);
            const tree = directoryTreeQueries[sessionIndex]?.data ?? directoryTreePlaceholder(session);
            return (
              <div
                key={session.rootPath}
                data-root-path={session.rootPath}
                className={`folder-root ${draggedRoot === session.rootPath ? "is-dragging" : ""} ${dropTargetRoot === session.rootPath && dropPlacement ? `is-drop-${dropPlacement}` : ""}`}
              >
                <DirectoryNode
                  session={session}
                  node={tree.root}
                  currentPath={activeSession?.rootPath === session.rootPath ? currentPath : undefined}
                  depth={0}
                  onNavigate={onNavigate}
                  onExpandedChange={changeDirectoryExpansion}
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
            );
          })}

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
                {t(contextMenu.session.deletionMode === "permanent" ? "deletePermanently" : "trashFolder")}
              </button>
            </div>,
            document.body,
          ) : null}

          {pendingTrash ? (
            <ConfirmTrashDialog
              deletionMode={pendingTrash.session.deletionMode}
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

        </div>

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
    </aside>
  );
}
