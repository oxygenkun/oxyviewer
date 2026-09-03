import { useVirtualizer } from "@tanstack/react-virtual";
import { Copy, FileImage, FolderOpen, Trash2 } from "lucide-react";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { MessageKey } from "../lib/i18n";
import { platformFileManager } from "../lib/folderPaths";
import { useWorkspaceStore } from "../store";
import type { AssetSummary, ViewMode } from "../types";
import { AssetMetadataBadges } from "./AssetMetadataBadges";
import { ConfirmTrashDialog } from "./ConfirmTrashDialog";
import { Loupe } from "./Loupe";
import { Thumbnail } from "./Thumbnail";

interface AssetBrowserProps {
  assets: AssetSummary[];
  total: number;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => void;
  onTrashAsset: (asset: AssetSummary) => void;
  onCopyAssetPath: (asset: AssetSummary, relative: boolean) => void;
  onOpenInFileManager: (path: string) => void;
  view: ViewMode;
  t: (key: MessageKey) => string;
}

interface AssetCardProps {
  asset: AssetSummary;
  priority: "nearby" | "visible";
  selected: boolean;
  onSelect: (event: React.MouseEvent) => void;
  onContextMenu: (event: React.MouseEvent) => void;
  onOpen: () => void;
  showMetadata: boolean;
}

const FILE_MANAGER_LABEL = {
  finder: "openInFinder",
  windowsExplorer: "openInWindowsExplorer",
  generic: "openInFileManager",
} as const satisfies Record<ReturnType<typeof platformFileManager>, MessageKey>;

const AssetCard = memo(function AssetCard({
  asset,
  priority,
  selected,
  onSelect,
  onContextMenu,
  onOpen,
  showMetadata,
}: AssetCardProps) {
  return (
    <button
      className={`asset-card ${selected ? "is-selected" : ""}`}
      onClick={onSelect}
      onDoubleClick={onOpen}
      title={asset.path}
    >
      <Thumbnail asset={asset} priority={priority} onContextMenu={onContextMenu} />
      <span className="asset-card__name">{asset.name}</span>
      <span className="asset-card__meta">
        {asset.extension}
        {showMetadata ? <AssetMetadataBadges asset={asset} /> : null}
        {asset.hasSidecar ? <i title="XMP sidecar" /> : null}
      </span>
    </button>
  );
});

export function AssetBrowser(props: AssetBrowserProps) {
  const [contextMenu, setContextMenu] = useState<{
    asset: AssetSummary;
    x: number;
    y: number;
  }>();
  const [pendingTrash, setPendingTrash] = useState<AssetSummary>();
  const select = useWorkspaceStore((state) => state.select);
  const showContextMenu = useCallback((event: React.MouseEvent, asset: AssetSummary) => {
    event.preventDefault();
    event.stopPropagation();
    select(asset.id);
    setContextMenu({
      asset,
      x: Math.max(8, Math.min(event.clientX, window.innerWidth - 224)),
      y: Math.max(8, Math.min(event.clientY, window.innerHeight - 132)),
    });
  }, [select]);

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

  let content;
  if (props.assets.length === 0) {
    content = (
      <div className="no-results">
        <FileImage size={31} strokeWidth={1.25} />
        <strong>{props.t("noResults")}</strong>
        <span>{props.t("noResultsBody")}</span>
      </div>
    );
  } else if (props.view === "loupe") {
    content = (
      <Loupe
        assets={props.assets}
        fetchNextPage={props.fetchNextPage}
        hasNextPage={props.hasNextPage}
        isFetchingNextPage={props.isFetchingNextPage}
        t={props.t}
      />
    );
  } else if (props.view === "list") {
    content = <VirtualList {...props} onAssetContextMenu={showContextMenu} />;
  } else {
    content = <VirtualGrid {...props} onAssetContextMenu={showContextMenu} />;
  }

  return (
    <>
      {content}
      {contextMenu ? createPortal(
        <div
          className="asset-context-menu"
          role="menu"
          aria-label={contextMenu.asset.name}
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onContextMenu={(event) => event.preventDefault()}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <button
            autoFocus
            role="menuitem"
            onClick={() => {
              const { asset } = contextMenu;
              setContextMenu(undefined);
              props.onCopyAssetPath(asset, true);
            }}
          >
            <Copy size={13} />
            {props.t("copyRelativePath")}
          </button>
          <button
            role="menuitem"
            onClick={() => {
              const { asset } = contextMenu;
              setContextMenu(undefined);
              props.onCopyAssetPath(asset, false);
            }}
          >
            <Copy size={13} />
            {props.t("copyAbsolutePath")}
          </button>
          <button
            role="menuitem"
            onClick={() => {
              const { asset } = contextMenu;
              setContextMenu(undefined);
              props.onOpenInFileManager(asset.path);
            }}
          >
            <FolderOpen size={13} />
            {props.t(FILE_MANAGER_LABEL[platformFileManager()])}
          </button>
          <div className="asset-context-menu__separator" />
          <button
            className="asset-context-menu__danger"
            role="menuitem"
            onClick={() => {
              const { asset } = contextMenu;
              setContextMenu(undefined);
              setPendingTrash(asset);
            }}
          >
            <Trash2 size={13} />
            {props.t("delete")}
          </button>
        </div>,
        document.body,
      ) : null}
      {pendingTrash ? (
        <ConfirmTrashDialog
          itemName={pendingTrash.name}
          onCancel={() => setPendingTrash(undefined)}
          onConfirm={() => {
            const asset = pendingTrash;
            setPendingTrash(undefined);
            props.onTrashAsset(asset);
          }}
          t={props.t}
        />
      ) : null}
    </>
  );
}

function VirtualGrid({
  assets,
  hasNextPage,
  isFetchingNextPage,
  fetchNextPage,
  onAssetContextMenu,
}: AssetBrowserProps & {
  onAssetContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(900);
  const selectedIds = useWorkspaceStore((state) => state.selectedIds);
  const select = useWorkspaceStore((state) => state.select);
  const setView = useWorkspaceStore((state) => state.setView);
  const gridPreference = useWorkspaceStore((state) => state.gridPreference);
  const gridMetadataVisible = useWorkspaceStore((state) => state.gridMetadataVisible);
  const portraitPriority = gridPreference === "portrait";
  const rowHeight = portraitPriority ? 274 : 194;
  const columns = Math.max(2, Math.floor(width / (portraitPriority ? 150 : 190)));
  const rowCount = Math.ceil(assets.length / columns);
  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 3,
  });
  const rows = virtualizer.getVirtualItems();

  useEffect(() => {
    if (!parentRef.current) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(parentRef.current);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    virtualizer.measure();
  }, [columns, rowHeight, virtualizer]);

  useEffect(() => {
    const last = rows.at(-1);
    if (last && last.index >= rowCount - 2 && hasNextPage && !isFetchingNextPage) fetchNextPage();
  }, [fetchNextPage, hasNextPage, isFetchingNextPage, rowCount, rows]);

  return (
    <div
      className={`asset-scroll virtual-grid--${gridPreference}`}
      ref={parentRef}
    >
      <div className="virtual-grid" style={{ height: virtualizer.getTotalSize() }}>
        {rows.map((row) => (
          <div
            className="virtual-grid__row"
            key={row.key}
            style={{
              gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
              height: rowHeight,
              transform: `translateY(${row.start}px)`,
            }}
          >
            {assets.slice(row.index * columns, row.index * columns + columns).map((asset) => (
              <AssetCard
                key={asset.id}
                asset={asset}
                priority={isVisible(row.start, row.end, parentRef.current) ? "visible" : "nearby"}
                selected={selectedIds.includes(asset.id)}
                onSelect={(event) => select(asset.id, event.metaKey || event.ctrlKey)}
                onContextMenu={(event) => onAssetContextMenu(event, asset)}
                onOpen={() => {
                  select(asset.id);
                  setView("loupe");
                }}
                showMetadata={gridMetadataVisible}
              />
            ))}
          </div>
        ))}
      </div>
      {isFetchingNextPage ? <div className="loading-more">Loading...</div> : null}
    </div>
  );
}

function VirtualList({
  assets,
  hasNextPage,
  isFetchingNextPage,
  fetchNextPage,
  t,
  onAssetContextMenu,
}: AssetBrowserProps & {
  onAssetContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const selectedIds = useWorkspaceStore((state) => state.selectedIds);
  const select = useWorkspaceStore((state) => state.select);
  const setView = useWorkspaceStore((state) => state.setView);
  const virtualizer = useVirtualizer({
    count: assets.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 58,
    overscan: 8,
  });
  const rows = virtualizer.getVirtualItems();

  useEffect(() => {
    const last = rows.at(-1);
    if (last && last.index >= assets.length - 10 && hasNextPage && !isFetchingNextPage) {
      fetchNextPage();
    }
  }, [assets.length, fetchNextPage, hasNextPage, isFetchingNextPage, rows]);

  return (
    <div className="asset-scroll asset-scroll--list" ref={parentRef}>
      <div className="list-header">
        <span>Name</span><span>{t("ratingAndTag")}</span><span>Type</span><span>Size</span><span>Modified</span>
      </div>
      <div className="virtual-list" style={{ height: virtualizer.getTotalSize() }}>
        {rows.map((row) => {
          const asset = assets[row.index];
          return (
            <button
              className={`asset-list-row ${selectedIds.includes(asset.id) ? "is-selected" : ""}`}
              key={row.key}
              style={{ transform: `translateY(${row.start}px)` }}
              onClick={(event) => select(asset.id, event.metaKey || event.ctrlKey)}
              onDoubleClick={() => setView("loupe")}
            >
              <Thumbnail
                asset={asset}
                priority={isVisible(row.start, row.end, parentRef.current) ? "visible" : "nearby"}
                onContextMenu={(event) => onAssetContextMenu(event, asset)}
              />
              <strong>{asset.name}</strong>
              <AssetMetadataBadges asset={asset} />
              <span>{asset.extension}</span>
              <span>{formatBytes(asset.sizeBytes)}</span>
              <span>{new Date(asset.modifiedAtMs).toLocaleDateString()}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function formatBytes(bytes: number) {
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(0)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

function isVisible(start: number, end: number, scrollElement: HTMLElement | null) {
  if (!scrollElement) return true;
  return end > scrollElement.scrollTop && start < scrollElement.scrollTop + scrollElement.clientHeight;
}
