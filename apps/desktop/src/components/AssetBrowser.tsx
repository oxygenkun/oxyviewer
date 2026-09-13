import { useVirtualizer } from "@tanstack/react-virtual";
import { FileImage } from "lucide-react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  activeAssetIndex,
  gridRowCount,
  gridRowForAsset,
  virtualAssetCount,
} from "../lib/assetViewPosition";
import { requestMetadata } from "../lib/api";
import type { MessageKey } from "../lib/i18n";
import { useExternalAppSettings } from "../lib/externalApps";
import { AssetContextMenu, type AssetMenuTarget } from "./AssetContextMenu";
import { acceptMetadataProjections } from "../lib/metadataProjection";
import { matchesAction } from "../lib/shortcuts";
import { useMarkingShortcuts } from "../lib/useMarkingShortcuts";
import {
  backgroundPreviewIntents,
  PreviewScheduleScope,
  viewportPreviewIntents,
} from "../lib/previewScheduling";
import { useWorkspaceStore } from "../store";
import type { AssetSummary, FileDeletionMode, ViewMode } from "../types";
import { AssetMetadataBadges } from "./AssetMetadataBadges";
import { ConfirmTrashDialog } from "./ConfirmTrashDialog";
import { Loupe } from "./Loupe";
import { Thumbnail } from "./Thumbnail";

interface AssetBrowserProps {
  assets: AssetSummary[];
  restoringActiveId?: string;
  total: number;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => void;
  onTrashAsset: (asset: AssetSummary) => void;
  onCopyAssetPath: (asset: AssetSummary, relative: boolean) => void;
  onOpenInFileManager: (path: string) => void;
  onOpenExternal: (path: string, appId?: string) => void;
  deletionMode: FileDeletionMode;
  view: ViewMode;
  t: (key: MessageKey) => string;
}

interface AssetCardProps {
  asset: AssetSummary;
  resourcesEnabled: boolean;
  priority: "nearby" | "visible";
  rank: number;
  selected: boolean;
  onSelect: (id: string, additive?: boolean) => void;
  onContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
  onOpen: (id: string) => void;
  showMetadata: boolean;
}

const AssetCard = memo(function AssetCard({
  asset,
  resourcesEnabled,
  priority,
  rank,
  selected,
  onSelect,
  onContextMenu,
  onOpen,
  showMetadata,
}: AssetCardProps) {
  const handleContextMenu = useCallback((event: React.MouseEvent) => {
    onContextMenu(event, asset);
  }, [onContextMenu, asset]);
  return (
    <button
      className={`asset-card ${selected ? "is-selected" : ""}`}
      onClick={(event) => onSelect(asset.id, event.metaKey || event.ctrlKey)}
      onDoubleClick={() => onOpen(asset.id)}
      title={asset.path}
    >
      <Thumbnail
        asset={asset}
        enabled={resourcesEnabled}
        priority={priority}
        rank={rank}
        onContextMenu={handleContextMenu}
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
  const [contextMenu, setContextMenu] = useState<AssetMenuTarget>();
  const externalApps = useExternalAppSettings();
  const openSettings = useWorkspaceStore((state) => state.openSettings);
  const dismissContextMenu = useCallback(() => setContextMenu(undefined), []);
  const [pendingTrash, setPendingTrash] = useState<AssetSummary>();
  const showContextMenu = useCallback((event: React.MouseEvent, asset: AssetSummary) => {
    event.preventDefault();
    event.stopPropagation();
    setContextMenu({
      asset,
      x: event.clientX,
      y: event.clientY,
    });
  }, []);

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
        restoringActiveId={props.restoringActiveId}
        total={props.total}
        fetchNextPage={props.fetchNextPage}
        hasNextPage={props.hasNextPage}
        isFetchingNextPage={props.isFetchingNextPage}
        keyboardSuppressed={Boolean(contextMenu) || Boolean(pendingTrash)}
        onAssetContextMenu={showContextMenu}
        t={props.t}
      />
    );
  } else if (props.view === "list") {
    content = (
      <VirtualList
        {...props}
        keyboardSuppressed={Boolean(contextMenu) || Boolean(pendingTrash)}
        onAssetContextMenu={showContextMenu}
      />
    );
  } else {
    content = (
      <VirtualGrid
        {...props}
        keyboardSuppressed={Boolean(contextMenu) || Boolean(pendingTrash)}
        onAssetContextMenu={showContextMenu}
      />
    );
  }

  return (
    <>
      {content}
      {contextMenu ? <AssetContextMenu
        key={`${contextMenu.asset.id}:${contextMenu.x}:${contextMenu.y}`}
        target={contextMenu}
        settings={externalApps.data}
        settingsError={externalApps.isError}
        deletionMode={props.deletionMode}
        t={props.t}
        onDismiss={dismissContextMenu}
        onOpen={props.onOpenExternal}
        onSettings={() => openSettings("externalApps")}
        onCopy={props.onCopyAssetPath}
        onReveal={props.onOpenInFileManager}
        onTrash={setPendingTrash}
      /> : null}
      {pendingTrash ? (
        <ConfirmTrashDialog
          deletionMode={props.deletionMode}
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
  keyboardSuppressed = false,
  onAssetContextMenu,
}: AssetBrowserProps & {
  keyboardSuppressed?: boolean;
  onAssetContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(900);
  const activeId = useWorkspaceStore((state) => state.activeId);
  const selectedIds = useWorkspaceStore((state) => state.selectedIds);
  const select = useWorkspaceStore((state) => state.select);
  const setView = useWorkspaceStore((state) => state.setView);
  const shortcuts = useWorkspaceStore((state) => state.shortcuts);
  const openAsset = useCallback((id: string) => {
    select(id);
    setView("loupe");
  }, [select, setView]);
  const thumbnailOrientation = useWorkspaceStore((state) => state.thumbnailOrientation);
  const gridMetadataVisible = useWorkspaceStore((state) => state.gridMetadataVisible);
  const usesPortraitThumbnails = thumbnailOrientation === "portrait";
  const rowHeight = usesPortraitThumbnails ? 274 : 194;
  const columns = Math.max(2, Math.floor(width / (usesPortraitThumbnails ? 150 : 190)));
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
    overscan: 6,
  });
  const rows = virtualizer.getVirtualItems();
  const resourcesEnabled = true;
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
  const visibleMetadataPaths = scheduleCandidates
    .filter((candidate) => candidate.visible)
    .map((candidate) => candidate.asset.path);
  const visibleMetadataSignature = visibleMetadataPaths.join("\u0000");
  useEffect(() => {
    if (!visibleMetadataPaths.length) return;
    void requestMetadata(visibleMetadataPaths, "visible")
      .then(acceptMetadataProjections)
      .catch(() => undefined);
  }, [visibleMetadataSignature]);
  useEffect(() => {
    // Visible and overscan requests remain active during scrolling. Rust
    // prioritizes the latest viewport and cancels requests after unmount.
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

  useMarkingShortcuts(assets, keyboardSuppressed);

  const stepSelection = useCallback((delta: number) => {
    const currentIndex = activeAssetIndex(assets, activeId);
    const nextIndex = currentIndex === undefined ? 0 : currentIndex + delta;
    const next = assets[nextIndex];
    if (next) {
      select(next.id);
      virtualizer.scrollToIndex(gridRowForAsset(nextIndex, columns), { align: "auto" });
    } else if (delta > 0 && hasNextPage && !isFetchingNextPage) {
      fetchNextPage();
    }
  }, [activeId, assets, columns, fetchNextPage, hasNextPage, isFetchingNextPage, select, virtualizer]);

  useEffect(() => {
    if (keyboardSuppressed) return;
    const editableTarget = (target: EventTarget | null) => (
      target instanceof HTMLElement
      && Boolean(target.closest("input, textarea, select, [contenteditable='true']"))
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (useWorkspaceStore.getState().settingsOpen || editableTarget(event.target)) return;
      if (matchesAction(event, shortcuts, "grid.moveLeft")) {
        event.preventDefault();
        stepSelection(-1);
      } else if (matchesAction(event, shortcuts, "grid.moveRight")) {
        event.preventDefault();
        stepSelection(1);
      } else if (matchesAction(event, shortcuts, "grid.moveUp")) {
        event.preventDefault();
        stepSelection(-columns);
      } else if (matchesAction(event, shortcuts, "grid.moveDown")) {
        event.preventDefault();
        stepSelection(columns);
      } else if (matchesAction(event, shortcuts, "grid.openLoupe")) {
        if (event.repeat) return;
        const target = activeId ?? assets[0]?.id;
        if (!target) return;
        event.preventDefault();
        openAsset(target);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeId, assets, columns, keyboardSuppressed, openAsset, shortcuts, stepSelection]);

  return (
    <div
      className={`asset-scroll virtual-grid--${thumbnailOrientation}`}
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
                const rank = Math.round(rowDistance / rowHeight) * columns + columnIndex;
                return asset ? (
                  <AssetCard
                    key={asset.id}
                    asset={asset}
                    priority={priority}
                    rank={rank}
                    selected={selectedIds.includes(asset.id)}
                    onSelect={select}
                    onContextMenu={onAssetContextMenu}
                    onOpen={openAsset}
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
  keyboardSuppressed = false,
  t,
  onAssetContextMenu,
}: AssetBrowserProps & {
  keyboardSuppressed?: boolean;
  onAssetContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const activeId = useWorkspaceStore((state) => state.activeId);
  const selectedIds = useWorkspaceStore((state) => state.selectedIds);
  const select = useWorkspaceStore((state) => state.select);
  const setView = useWorkspaceStore((state) => state.setView);
  const shortcuts = useWorkspaceStore((state) => state.shortcuts);
  const thumbnailOrientation = useWorkspaceStore((state) => state.thumbnailOrientation);
  const rowHeight = thumbnailOrientation === "portrait" ? 68 : 58;
  const assetCount = virtualAssetCount(assets.length, total);
  const virtualizer = useVirtualizer({
    count: assetCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 16,
  });
  const rows = virtualizer.getVirtualItems();
  const resourcesEnabled = true;
  const viewportCenter = (parentRef.current?.scrollTop ?? 0)
    + (parentRef.current?.clientHeight ?? rowHeight) / 2;
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
  const visibleMetadataPaths = scheduleCandidates
    .filter((candidate) => candidate.visible)
    .map((candidate) => candidate.asset.path);
  const visibleMetadataSignature = visibleMetadataPaths.join("\u0000");
  useEffect(() => {
    if (!visibleMetadataPaths.length) return;
    void requestMetadata(visibleMetadataPaths, "visible")
      .then(acceptMetadataProjections)
      .catch(() => undefined);
  }, [visibleMetadataSignature]);
  useEffect(() => {
    // Visible and overscan requests remain active during scrolling. Rust
    // prioritizes the latest viewport and cancels requests after unmount.
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
    virtualizer.measure();
  }, [rowHeight, virtualizer]);

  useEffect(() => {
    const last = rows.at(-1);
    if (last && last.index >= assets.length - 10 && hasNextPage && !isFetchingNextPage) {
      fetchNextPage();
    }
  }, [assets.length, fetchNextPage, hasNextPage, isFetchingNextPage, rows]);

  useEffect(() => {
    if (keyboardSuppressed) return;
    const editableTarget = (target: EventTarget | null) => (
      target instanceof HTMLElement
      && Boolean(target.closest("input, textarea, select, [contenteditable='true']"))
    );
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.repeat) return;
      if (useWorkspaceStore.getState().settingsOpen || editableTarget(event.target)) return;
      if (!matchesAction(event, shortcuts, "grid.openLoupe")) return;
      const target = activeId ?? assets[0]?.id;
      if (!target) return;
      event.preventDefault();
      select(target);
      setView("loupe");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeId, assets, keyboardSuppressed, select, setView, shortcuts]);

  return (
    <div
      className={`asset-scroll asset-scroll--list virtual-list--${thumbnailOrientation}`}
      ref={parentRef}
    >
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
                style={{ height: rowHeight, transform: `translateY(${row.start}px)` }}
              />
            );
          }
          return (
            <button
              className={`asset-list-row ${selectedIds.includes(asset.id) ? "is-selected" : ""}`}
              key={row.key}
              style={{ height: rowHeight, transform: `translateY(${row.start}px)` }}
              onClick={(event) => select(asset.id, event.metaKey || event.ctrlKey)}
              onDoubleClick={() => setView("loupe")}
            >
              <Thumbnail
                asset={asset}
                enabled={resourcesEnabled}
                priority={isVisible(row.start, row.end, parentRef.current) ? "visible" : "nearby"}
                rank={Math.round(Math.abs((row.start + row.end) / 2 - viewportCenter) / rowHeight)}
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
