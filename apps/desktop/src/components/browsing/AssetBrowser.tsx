import { useVirtualizer } from "@tanstack/react-virtual";
import { FileImage } from "lucide-react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  activeAssetIndex,
  gridRowCount,
  gridRowForAsset,
  virtualAssetCount,
} from "@/lib/assets/assetViewPosition";
import { requestMetadata } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { useExternalAppSettings } from "@/lib/media/externalApps";
import { AssetContextMenu, type AssetMenuTarget } from "./AssetContextMenu";
import { acceptMetadataProjections } from "@/lib/projection/metadataProjection";
import { matchesAction } from "@/lib/ui/shortcuts";
import { useMarkingShortcuts } from "@/lib/hooks/useMarkingShortcuts";
import {
  backgroundPreviewIntents,
  PreviewScheduleScope,
  viewportPreviewIntents,
} from "@/lib/preview/previewScheduling";
import { useOrientationRetention } from "@/lib/hooks/useOrientationRetention";
import { useWorkspaceStore } from "@/store";
import { burstAdjustedTotal, useBurstGroups } from "@/lib/browse/burstGroups";
import type { AssetSummary, BurstGroup, FileDeletionMode, ViewMode } from "@/types";
import { AssetMetadataBadges } from "@/components/common/AssetMetadataBadges";
import { ConfirmTrashDialog } from "@/components/overlay/ConfirmTrashDialog";
import { Loupe } from "@/components/loupe/Loupe";
import { Thumbnail } from "./Thumbnail";

interface AssetBrowserProps {
  assets: AssetSummary[];
  restoringActiveId?: string;
  total: number;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => void;
  onTrashAssets: (assets: AssetSummary[]) => void;
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
  focused: boolean;
  onSelect: (id: string, additive: boolean, range: boolean) => void;
  onContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
  onOpen: (id: string) => void;
  showMetadata: boolean;
  /** Burst this tile belongs to; collapsed groups expose only their first frame. */
  burst?: BurstGroup;
  burstExpanded?: boolean;
  burstMemberIndex?: number;
  expandBurstLabel: string;
  collapseBurstLabel: string;
  collapseBurstText: string;
  onToggleBurst?: (representative: string) => void;
}

const AssetCard = memo(function AssetCard({
  asset,
  resourcesEnabled,
  priority,
  rank,
  selected,
  focused,
  onSelect,
  onContextMenu,
  onOpen,
  showMetadata,
  burst,
  burstExpanded = false,
  burstMemberIndex,
  expandBurstLabel,
  collapseBurstLabel,
  collapseBurstText,
  onToggleBurst,
}: AssetCardProps) {
  const expandedFromCollapsedAtRef = useRef(Number.NEGATIVE_INFINITY);
  const handleContextMenu = useCallback((event: React.MouseEvent) => {
    onContextMenu(event, asset);
  }, [onContextMenu, asset]);
  const handleBurstToggle = useCallback((event: React.MouseEvent) => {
    event.stopPropagation();
    if (burst) onToggleBurst?.(burst.representative);
  }, [burst, onToggleBurst]);
  const handleSelect = useCallback((event: React.MouseEvent) => {
    if (burst && !burstExpanded) {
      expandedFromCollapsedAtRef.current = event.timeStamp;
      onToggleBurst?.(burst.representative);
    }
    onSelect(asset.id, event.metaKey || event.ctrlKey, event.shiftKey);
  }, [asset.id, burst, burstExpanded, onSelect, onToggleBurst]);
  const handleOpen = useCallback((event: React.MouseEvent) => {
    const expandedDuringThisDoubleClick = event.timeStamp - expandedFromCollapsedAtRef.current < 500;
    if (burst && (!burstExpanded || expandedDuringThisDoubleClick)) return;
    onOpen(asset.id);
  }, [asset.id, burst, burstExpanded, onOpen]);
  const burstClassName = burstExpanded
    ? ` is-burst-expanded${burstMemberIndex === 0 ? " is-burst-first" : ""}${
      burstMemberIndex === (burst?.members.length ?? 0) - 1 ? " is-burst-last" : ""
    }`
    : "";
  return (
    <button
      className={`asset-card${selected ? " is-selected" : ""}${focused ? " is-active" : ""}${burstClassName}`}
      onClick={handleSelect}
      onDoubleClick={handleOpen}
      title={asset.path}
    >
      <Thumbnail
        asset={asset}
        enabled={resourcesEnabled}
        priority={priority}
        rank={rank}
        onContextMenu={handleContextMenu}
      />
      {burst ? (
        <span
          className={`asset-card__burst ${burstExpanded ? "is-expanded" : ""}`}
          onClick={handleBurstToggle}
          title={burstExpanded ? collapseBurstLabel : expandBurstLabel}
          aria-label={burstExpanded ? collapseBurstLabel : expandBurstLabel}
        >
          {burstExpanded ? collapseBurstText : `×${burst.members.length}`}
        </span>
      ) : null}
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
  const select = useWorkspaceStore((state) => state.select);
  const dismissContextMenu = useCallback(() => setContextMenu(undefined), []);
  const [pendingTrash, setPendingTrash] = useState<AssetSummary[]>();
  const showContextMenu = useCallback((event: React.MouseEvent, asset: AssetSummary) => {
    event.preventDefault();
    event.stopPropagation();
    if (!useWorkspaceStore.getState().selectedIds.includes(asset.id)) select(asset.id);
    setContextMenu({
      asset,
      x: event.clientX,
      y: event.clientY,
    });
  }, [select]);
  const requestTrash = useCallback((asset: AssetSummary) => {
    const selected = new Set(useWorkspaceStore.getState().selectedIds);
    const targets = selected.has(asset.id)
      ? props.assets.filter((candidate) => selected.has(candidate.id))
      : [asset];
    setPendingTrash(targets.length ? targets : [asset]);
  }, [props.assets]);

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
        selectionCount={useWorkspaceStore.getState().selectedIds.length}
        onTrash={requestTrash}
      /> : null}
      {pendingTrash ? (
        <ConfirmTrashDialog
          deletionMode={props.deletionMode}
          itemCount={pendingTrash.length}
          itemName={pendingTrash[0].name}
          onCancel={() => setPendingTrash(undefined)}
          onConfirm={() => {
            const assets = pendingTrash;
            setPendingTrash(undefined);
            props.onTrashAssets(assets);
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
  t,
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
  const selectRange = useWorkspaceStore((state) => state.selectRange);
  const setView = useWorkspaceStore((state) => state.setView);
  const shortcuts = useWorkspaceStore((state) => state.shortcuts);
  const openAsset = useCallback((id: string) => {
    select(id);
    setView("loupe");
  }, [select, setView]);
  const thumbnailOrientation = useWorkspaceStore((state) => state.thumbnailOrientation);
  const burstGroupingEnabled = useWorkspaceStore((state) => state.burstGroupingEnabled);
  const gridMetadataVisible = useWorkspaceStore((state) => state.gridMetadataVisible);
  // Bursts collapse in the grid only: the loupe keeps every frame, which is
  // the whole point of shooting a burst.
  const { visible: gridAssets, byMember, expanded, toggle } = useBurstGroups(
    assets,
    burstGroupingEnabled,
  );
  const gridAssetIds = useMemo(() => gridAssets.map((asset) => asset.id), [gridAssets]);
  const handleSelect = useCallback((id: string, additive: boolean, range: boolean) => {
    if (range) selectRange(gridAssetIds, id, additive);
    else select(id, additive);
  }, [gridAssetIds, select, selectRange]);
  const usesPortraitThumbnails = thumbnailOrientation === "portrait";
  const rowHeight = usesPortraitThumbnails ? 274 : 194;
  const columns = Math.max(2, Math.floor(width / (usesPortraitThumbnails ? 150 : 190)));
  const adjustedTotal = burstAdjustedTotal(total, assets.length, gridAssets.length);
  const assetCount = virtualAssetCount(gridAssets.length, adjustedTotal);
  const rowCount = gridRowCount(assetCount, columns);
  const loadedRowCount = gridRowCount(gridAssets.length, columns);
  const restoreActiveId = useRef(activeId).current;
  const restoreAssetIndex = activeAssetIndex(gridAssets, restoreActiveId);
  const restoreRowIndex = restoreAssetIndex === undefined
    ? undefined
    : gridRowForAsset(restoreAssetIndex, columns);
  const restoreApplied = useRef(false);
  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 6,
  });
  const rows = virtualizer.getVirtualItems();
  const collapsingRepresentativeRef = useRef<string | undefined>(undefined);
  const toggleBurst = useCallback((representative: string) => {
    if (expanded.has(representative)) {
      collapsingRepresentativeRef.current = representative;
      const representativeAsset = assets.find((asset) => asset.path === representative);
      if (representativeAsset) select(representativeAsset.id);
    }
    toggle(representative);
  }, [assets, expanded, select, toggle]);
  const resourcesEnabled = true;
  const viewportCenter = (parentRef.current?.scrollTop ?? 0)
    + (parentRef.current?.clientHeight ?? rowHeight) / 2;
  const selectedAsset = gridAssets.find((asset) => asset.id === activeId);
  const [viewportSchedule] = useState(() => new PreviewScheduleScope("grid-viewport"));
  const [backgroundSchedule] = useState(() => new PreviewScheduleScope(
    "grid-background",
    { minDispatchIntervalMs: 200 },
  ));
  const scheduleCandidates = useMemo(() => rows.flatMap((row) => {
    const visible = isVisible(row.start, row.end, parentRef.current);
    const distance = Math.abs((row.start + row.end) / 2 - viewportCenter);
    return Array.from({ length: columns }, (_, columnIndex) => {
      const asset = gridAssets[row.index * columns + columnIndex];
      return asset ? { asset, visible, distance: distance + columnIndex } : undefined;
    }).filter((candidate) => candidate !== undefined);
  }), [gridAssets, columns, rows, viewportCenter]);
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
      backgroundSchedule.reconcile(backgroundPreviewIntents(gridAssets, activeId));
    }, 150);
    return () => window.clearTimeout(timer);
  }, [activeId, gridAssets, backgroundSchedule]);

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
    // Center the photo restored from the folder exactly once. Later column
    // changes belong to the current selection, not to the restored one.
    if (restoreApplied.current || restoreRowIndex === undefined) return;
    restoreApplied.current = true;
    virtualizer.scrollToIndex(restoreRowIndex, { align: "center" });
  }, [restoreRowIndex, virtualizer]);

  useLayoutEffect(() => {
    const representative = collapsingRepresentativeRef.current;
    if (!representative) return;
    const representativeIndex = gridAssets.findIndex((asset) => asset.path === representative);
    if (representativeIndex < 0) return;
    collapsingRepresentativeRef.current = undefined;
    virtualizer.measure();
    const representativeRow = gridRowForAsset(representativeIndex, columns);
    const rowStart = representativeRow * rowHeight;
    if (!isVisible(rowStart, rowStart + rowHeight, parentRef.current)) {
      virtualizer.scrollToIndex(representativeRow, { align: "start" });
    }
  }, [columns, gridAssets, rowHeight, virtualizer]);

  useOrientationRetention(thumbnailOrientation, () => {
    // Columns and row height both change with the orientation, so re-measure
    // before scrolling the active photo back inside the viewport.
    virtualizer.measure();
    const activeIndex = activeAssetIndex(gridAssets, activeId);
    if (activeIndex === undefined) return;
    virtualizer.scrollToIndex(gridRowForAsset(activeIndex, columns), { align: "auto" });
  });

  useEffect(() => {
    const last = rows.at(-1);
    if (last && last.index >= loadedRowCount - 2 && hasNextPage && !isFetchingNextPage) {
      fetchNextPage();
    }
  }, [fetchNextPage, hasNextPage, isFetchingNextPage, loadedRowCount, rows]);

  useMarkingShortcuts(gridAssets, keyboardSuppressed);

  const stepSelection = useCallback((delta: number) => {
    const currentIndex = activeAssetIndex(gridAssets, activeId);
    const nextIndex = currentIndex === undefined ? 0 : currentIndex + delta;
    const next = gridAssets[nextIndex];
    if (next) {
      select(next.id);
      virtualizer.scrollToIndex(gridRowForAsset(nextIndex, columns), { align: "auto" });
    } else if (delta > 0 && hasNextPage && !isFetchingNextPage) {
      fetchNextPage();
    }
  }, [activeId, gridAssets, columns, fetchNextPage, hasNextPage, isFetchingNextPage, select, virtualizer]);

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
        const target = activeId ?? gridAssets[0]?.id;
        if (!target) return;
        event.preventDefault();
        openAsset(target);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeId, gridAssets, columns, keyboardSuppressed, openAsset, shortcuts, stepSelection]);

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
                const asset = gridAssets[assetIndex];
                const priority = isVisible(row.start, row.end, parentRef.current)
                  ? "visible"
                  : "nearby";
                const rowDistance = Math.abs((row.start + row.end) / 2 - viewportCenter);
                const rank = Math.round(rowDistance / rowHeight) * columns + columnIndex;
                const burst = asset ? byMember.get(asset.path) : undefined;
                const burstExpanded = burst ? expanded.has(burst.representative) : false;
                return asset ? (
                  <AssetCard
                    key={asset.id}
                    asset={asset}
                    priority={priority}
                    rank={rank}
                    selected={selectedIds.includes(asset.id)}
                    focused={activeId === asset.id}
                    onSelect={handleSelect}
                    onContextMenu={onAssetContextMenu}
                    onOpen={openAsset}
                    showMetadata={gridMetadataVisible}
                    resourcesEnabled={resourcesEnabled}
                    burst={burst}
                    burstExpanded={burstExpanded}
                    burstMemberIndex={burst?.members.indexOf(asset.path)}
                    expandBurstLabel={t("expandBurst")}
                    collapseBurstLabel={t("collapseBurst")}
                    collapseBurstText={t("collapse")}
                    onToggleBurst={toggleBurst}
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

export function formatBytes(bytes: number) {
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(0)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

function isVisible(start: number, end: number, scrollElement: HTMLElement | null) {
  if (!scrollElement) return true;
  return end > scrollElement.scrollTop && start < scrollElement.scrollTop + scrollElement.clientHeight;
}
