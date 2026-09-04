import { useVirtualizer } from "@tanstack/react-virtual";
import { Copy, FileImage, FolderOpen, Trash2 } from "lucide-react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  activeAssetIndex,
  gridRowCount,
  gridRowForAsset,
  virtualAssetCount,
} from "../lib/assetViewPosition";
import type { MessageKey } from "../lib/i18n";
import { platformFileManager } from "../lib/folderPaths";
import {
  backgroundPreviewIntents,
  PreviewScheduleScope,
  viewportPreviewIntents,
} from "../lib/previewScheduling";
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
  resourcesEnabled: boolean;
  priority: "nearby" | "visible";
  queueOrder: number;
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

// Keep filesystem fetches and webview image decodes out of active scroll
// frames. The virtualizer still paints summaries/placeholders immediately and
// re-enables resource work once the viewport has settled.
const RESOURCE_LOAD_SCROLL_IDLE_MS = 160;

const AssetCard = memo(function AssetCard({
  asset,
  resourcesEnabled,
  priority,
  queueOrder,
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
      <Thumbnail
        asset={asset}
        enabled={resourcesEnabled}
        priority={priority}
        queueOrder={queueOrder}
        onContextMenu={onContextMenu}
      />
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
  const showContextMenu = useCallback((event: React.MouseEvent, asset: AssetSummary) => {
    event.preventDefault();
    event.stopPropagation();
    setContextMenu({
      asset,
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
        total={props.total}
        fetchNextPage={props.fetchNextPage}
        hasNextPage={props.hasNextPage}
        isFetchingNextPage={props.isFetchingNextPage}
        onAssetContextMenu={showContextMenu}
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
  total,
  hasNextPage,
  isFetchingNextPage,
  fetchNextPage,
  onAssetContextMenu,
}: AssetBrowserProps & {
  onAssetContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(900);
  const activeId = useWorkspaceStore((state) => state.activeId);
  const selectedIds = useWorkspaceStore((state) => state.selectedIds);
  const select = useWorkspaceStore((state) => state.select);
  const setView = useWorkspaceStore((state) => state.setView);
  const gridPreference = useWorkspaceStore((state) => state.gridPreference);
  const gridMetadataVisible = useWorkspaceStore((state) => state.gridMetadataVisible);
  const portraitPriority = gridPreference === "portrait";
  const rowHeight = portraitPriority ? 274 : 194;
  const columns = Math.max(2, Math.floor(width / (portraitPriority ? 150 : 190)));
  const assetCount = virtualAssetCount(assets.length, total);
  const rowCount = gridRowCount(assetCount, columns);
  const loadedRowCount = gridRowCount(assets.length, columns);
  const restoreActiveId = useRef(activeId).current;
  const restoreAssetIndex = activeAssetIndex(assets, restoreActiveId);
  const restoreRowIndex = restoreAssetIndex === undefined
    ? undefined
    : gridRowForAsset(restoreAssetIndex, columns);
  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 3,
    isScrollingResetDelay: RESOURCE_LOAD_SCROLL_IDLE_MS,
  });
  const rows = virtualizer.getVirtualItems();
  const resourcesEnabled = !virtualizer.isScrolling;
  const viewportCenter = (parentRef.current?.scrollTop ?? 0)
    + (parentRef.current?.clientHeight ?? rowHeight) / 2;
  const selectedAsset = assets.find((asset) => asset.id === activeId);
  const [viewportSchedule] = useState(() => new PreviewScheduleScope("grid-viewport"));
  const [backgroundSchedule] = useState(() => new PreviewScheduleScope(
    "grid-background",
    { minDispatchIntervalMs: 200 },
  ));
  const scheduleCandidates = useMemo(() => rows.flatMap((row) => {
    const visible = isVisible(row.start, row.end, parentRef.current);
    const distance = Math.abs((row.start + row.end) / 2 - viewportCenter);
    return Array.from({ length: columns }, (_, columnIndex) => {
      const asset = assets[row.index * columns + columnIndex];
      return asset ? { asset, visible, distance: distance + columnIndex } : undefined;
    }).filter((candidate) => candidate !== undefined);
  }), [assets, columns, rows, viewportCenter]);
  const viewportIntents = useMemo(
    () => viewportPreviewIntents(scheduleCandidates, selectedAsset),
    [scheduleCandidates, selectedAsset],
  );
  useEffect(() => {
    // Keep the cheap native schedule synchronized while scrolling. Thumbnail
    // queries and WebView image decodes remain paused by `resourcesEnabled`,
    // but the latest viewport is already prioritized when scrolling settles.
    viewportSchedule.reconcile(viewportIntents);
  }, [viewportIntents, viewportSchedule]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      backgroundSchedule.reconcile(backgroundPreviewIntents(assets, activeId));
    }, 150);
    return () => window.clearTimeout(timer);
  }, [activeId, assets, backgroundSchedule]);

  useEffect(() => () => {
    viewportSchedule.release();
    backgroundSchedule.release();
  }, [backgroundSchedule, viewportSchedule]);

  useEffect(() => {
    if (!parentRef.current) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(parentRef.current);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    virtualizer.measure();
  }, [columns, rowHeight, virtualizer]);

  useLayoutEffect(() => {
    if (restoreRowIndex === undefined) return;
    virtualizer.scrollToIndex(restoreRowIndex, { align: "center" });
  }, [restoreRowIndex, virtualizer]);

  useEffect(() => {
    const last = rows.at(-1);
    if (last && last.index >= loadedRowCount - 2 && hasNextPage && !isFetchingNextPage) {
      fetchNextPage();
    }
  }, [fetchNextPage, hasNextPage, isFetchingNextPage, loadedRowCount, rows]);

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
            {Array.from(
              { length: Math.min(columns, assetCount - row.index * columns) },
              (_, columnIndex) => {
                const assetIndex = row.index * columns + columnIndex;
                const asset = assets[assetIndex];
                const priority = isVisible(row.start, row.end, parentRef.current)
                  ? "visible"
                  : "nearby";
                const rowDistance = Math.abs((row.start + row.end) / 2 - viewportCenter);
                const queueOrder = Math.round(rowDistance / rowHeight) * columns + columnIndex;
                return asset ? (
                  <AssetCard
                    key={asset.id}
                    asset={asset}
                    priority={priority}
                    queueOrder={queueOrder}
                    selected={selectedIds.includes(asset.id)}
                    onSelect={(event) => select(asset.id, event.metaKey || event.ctrlKey)}
                    onContextMenu={(event) => onAssetContextMenu(event, asset)}
                    onOpen={() => {
                      select(asset.id);
                      setView("loupe");
                    }}
                    showMetadata={gridMetadataVisible}
                    resourcesEnabled={resourcesEnabled}
                  />
                ) : (
                  <div className="asset-card-placeholder" key={`placeholder-${assetIndex}`} aria-hidden="true" />
                );
              },
            )}
          </div>
        ))}
      </div>
      {isFetchingNextPage ? <div className="loading-more">Loading...</div> : null}
    </div>
  );
}

function VirtualList({
  assets,
  total,
  hasNextPage,
  isFetchingNextPage,
  fetchNextPage,
  t,
  onAssetContextMenu,
}: AssetBrowserProps & {
  onAssetContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const activeId = useWorkspaceStore((state) => state.activeId);
  const selectedIds = useWorkspaceStore((state) => state.selectedIds);
  const select = useWorkspaceStore((state) => state.select);
  const setView = useWorkspaceStore((state) => state.setView);
  const assetCount = virtualAssetCount(assets.length, total);
  const virtualizer = useVirtualizer({
    count: assetCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 58,
    overscan: 8,
    isScrollingResetDelay: RESOURCE_LOAD_SCROLL_IDLE_MS,
  });
  const rows = virtualizer.getVirtualItems();
  const resourcesEnabled = !virtualizer.isScrolling;
  const viewportCenter = (parentRef.current?.scrollTop ?? 0)
    + (parentRef.current?.clientHeight ?? 58) / 2;
  const selectedAsset = assets.find((asset) => asset.id === activeId);
  const [viewportSchedule] = useState(() => new PreviewScheduleScope("list-viewport"));
  const [backgroundSchedule] = useState(() => new PreviewScheduleScope(
    "list-background",
    { minDispatchIntervalMs: 200 },
  ));
  const scheduleCandidates = useMemo(() => rows.flatMap((row) => {
    const asset = assets[row.index];
    return asset ? [{
      asset,
      visible: isVisible(row.start, row.end, parentRef.current),
      distance: Math.abs((row.start + row.end) / 2 - viewportCenter),
    }] : [];
  }), [assets, rows, viewportCenter]);
  const viewportIntents = useMemo(
    () => viewportPreviewIntents(scheduleCandidates, selectedAsset),
    [scheduleCandidates, selectedAsset],
  );
  useEffect(() => {
    // Keep the cheap native schedule synchronized while scrolling. Thumbnail
    // queries and WebView image decodes remain paused by `resourcesEnabled`,
    // but the latest viewport is already prioritized when scrolling settles.
    viewportSchedule.reconcile(viewportIntents);
  }, [viewportIntents, viewportSchedule]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      backgroundSchedule.reconcile(backgroundPreviewIntents(assets, activeId));
    }, 150);
    return () => window.clearTimeout(timer);
  }, [activeId, assets, backgroundSchedule]);

  useEffect(() => () => {
    viewportSchedule.release();
    backgroundSchedule.release();
  }, [backgroundSchedule, viewportSchedule]);
  const restoreActiveId = useRef(activeId).current;
  const restoreAssetIndex = activeAssetIndex(assets, restoreActiveId);

  useLayoutEffect(() => {
    if (restoreAssetIndex === undefined) return;
    virtualizer.scrollToIndex(restoreAssetIndex, { align: "center" });
  }, [restoreAssetIndex, virtualizer]);

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
          if (!asset) {
            return (
              <div
                aria-hidden="true"
                className="asset-list-row asset-list-row--placeholder"
                key={row.key}
                style={{ transform: `translateY(${row.start}px)` }}
              />
            );
          }
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
                enabled={resourcesEnabled}
                priority={isVisible(row.start, row.end, parentRef.current) ? "visible" : "nearby"}
                queueOrder={Math.round(Math.abs((row.start + row.end) / 2 - viewportCenter) / 58)}
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
