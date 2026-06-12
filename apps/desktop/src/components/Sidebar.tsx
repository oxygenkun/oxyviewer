import { useQuery } from "@tanstack/react-query";
import {
  Bookmark,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  Images,
  Library,
  LoaderCircle,
  Plus,
  RefreshCw,
  Settings,
} from "lucide-react";
import { useState } from "react";
import { listDirectories } from "../lib/api";
import type { MessageKey } from "../lib/i18n";
import type { DirectorySummary, FolderSession } from "../types";

interface SidebarProps {
  session: FolderSession;
  currentPath: string;
  libraryRoots: string[];
  onOpen: () => void;
  onNavigate: (path: string) => void;
  onRefresh: () => void;
  isRefreshing: boolean;
  onAddLibrary: () => void;
  onSettings: () => void;
  t: (key: MessageKey) => string;
}

interface DirectoryNodeProps {
  sessionId: string;
  entry: DirectorySummary;
  currentPath: string;
  depth: number;
  initiallyExpanded?: boolean;
  onNavigate: (path: string) => void;
}

function basename(path: string) {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

function DirectoryNode({
  sessionId,
  entry,
  currentPath,
  depth,
  initiallyExpanded = false,
  onNavigate,
}: DirectoryNodeProps) {
  const [expanded, setExpanded] = useState(initiallyExpanded);
  const children = useQuery({
    queryKey: ["directories", sessionId, entry.path],
    queryFn: () => listDirectories(sessionId, entry.path),
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
          onClick={() => onNavigate(entry.path)}
          title={entry.path}
        >
          {isActive ? <FolderOpen size={15} /> : <Folder size={15} />}
          <span>{entry.name}</span>
          {isActive ? <i /> : null}
        </button>
      </div>
      {expanded ? (
        <div className="directory-node__children">
          {(children.data ?? []).map((child) => (
            <DirectoryNode
              key={child.path}
              sessionId={sessionId}
              entry={child}
              currentPath={currentPath}
              depth={depth + 1}
              onNavigate={onNavigate}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

export function Sidebar({
  session,
  currentPath,
  libraryRoots,
  onOpen,
  onNavigate,
  onRefresh,
  isRefreshing,
  onAddLibrary,
  onSettings,
  t,
}: SidebarProps) {
  const root: DirectorySummary = {
    path: session.rootPath,
    name: session.displayName,
    hasChildren: true,
  };

  return (
    <aside className="sidebar">
      <div className="sidebar__brand">
        <span className="brand-glyph">OX</span>
        <div><strong>OxyViewer</strong><small>PHOTO DESK</small></div>
      </div>

      <div className="sidebar__section sidebar__section--folders">
        <div className="sidebar__heading">
          <span>{t("folders")}</span>
          <span className="sidebar__heading-actions">
            <button
              title={t("refreshFolder")}
              aria-label={t("refreshFolder")}
              disabled={isRefreshing}
              onClick={onRefresh}
            >
              <RefreshCw className={isRefreshing ? "tree-row__loader" : undefined} size={13} />
            </button>
            <button title={t("openFolder")} onClick={onOpen}><Plus size={14} /></button>
          </span>
        </div>
        <DirectoryNode
          key={session.id}
          sessionId={session.id}
          entry={root}
          currentPath={currentPath}
          depth={0}
          initiallyExpanded
          onNavigate={onNavigate}
        />
      </div>

      <div className="sidebar__section">
        <div className="sidebar__heading">
          <span>{t("libraries")}</span>
          <button title={t("addLibrary")} onClick={onAddLibrary}><Plus size={14} /></button>
        </div>
        <button className="tree-row">
          <Images size={15} />
          <span>All photos</span>
        </button>
        <button className="tree-row">
          <Bookmark size={15} />
          <span>5 star selects</span>
        </button>
        {libraryRoots.map((rootPath) => (
          <button className="tree-row" key={rootPath} title={rootPath}>
            <Library size={15} />
            <span>{basename(rootPath)}</span>
          </button>
        ))}
        {libraryRoots.length === 0 ? (
          <button className="sidebar__library-prompt" onClick={onAddLibrary}>
            <Library size={16} />
            <span>{t("addLibrary")}</span>
          </button>
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
      <div className="sidebar__path" title={currentPath}>{currentPath}</div>
    </aside>
  );
}
