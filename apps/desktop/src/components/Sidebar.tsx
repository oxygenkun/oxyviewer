import { useQuery } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  LoaderCircle,
  Plus,
  RefreshCw,
  Settings,
  X,
} from "lucide-react";
import { useState } from "react";
import { listDirectories } from "../lib/api";
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

        {sessions.map((session) => (
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
