import { useVirtualizer } from "@tanstack/react-virtual";
import { useQuery } from "@tanstack/react-query";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { FILMSTRIP_GAP, filmstripItemWidth, orderVisibleFilmstripItems, shouldFetchFilmstripPage } from "../lib/loupe";
import { requestMetadata } from "../lib/api";
import { getBatchedAssetTags } from "../lib/assetTagBatch";
import { acceptMetadataProjections } from "../lib/metadataProjection";
import { filmstripPreviewIntents, PreviewScheduleScope } from "../lib/previewScheduling";
import { useWorkspaceStore } from "../store";
import type { AssetSummary } from "../types";
import { AssetMetadataBadges } from "./AssetMetadataBadges";
import { FilmstripPreviewPreloader } from "./FilmstripPreviewPreloader";
import { Thumbnail } from "./Thumbnail";
import { installFilmstripWheel } from "../lib/filmstripWheel";
import { useOrientationRetention } from "../lib/useOrientationRetention";

interface FilmstripProps {
  active: AssetSummary;
  assets: AssetSummary[];
  nearbyPreviewAssets: AssetSummary[];
  total: number;
  fetchNextPage: () => void;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  onAssetContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
}

export const Filmstrip = memo(function Filmstrip({ active, assets, nearbyPreviewAssets, total, fetchNextPage,
  hasNextPage, isFetchingNextPage, onAssetContextMenu }: FilmstripProps) {
  const select = useWorkspaceStore((state) => state.select);
  const filmstripHeight = useWorkspaceStore((state) => state.filmstripHeight);
  const thumbnailOrientation = useWorkspaceStore((state) => state.thumbnailOrientation);
  const loupeMetadataVisible = useWorkspaceStore((state) => state.loupeMetadataVisible);
  const filmstripRef = useRef<HTMLDivElement>(null);
  const activeIndex = assets.indexOf(active);
  const [filmstripSchedule] = useState(() => new PreviewScheduleScope("loupe-filmstrip"));
  const filmstripItemSize = filmstripItemWidth(filmstripHeight, thumbnailOrientation);
  const filmstripVirtualizer = useVirtualizer({
    count: total,
    estimateSize: () => filmstripItemSize,
    gap: FILMSTRIP_GAP,
    getScrollElement: () => filmstripRef.current,
    horizontal: true,
    overscan: 12,
  });
  const virtualFilmstripItems = filmstripVirtualizer.getVirtualItems();
  const filmstripViewportStart = filmstripRef.current?.scrollLeft ?? 0;
  const filmstripViewportEnd = filmstripViewportStart + (filmstripRef.current?.clientWidth ?? 0);
  const nextVisibleFilmstripIds = orderVisibleFilmstripItems(
    virtualFilmstripItems.flatMap((item) => {
      const asset = assets[item.index];
      return asset ? [{ id: asset.id, start: item.start, end: item.end }] : [];
    }),
    filmstripViewportStart,
    filmstripViewportEnd,
  );
  const filmstripIdsRef = useRef(nextVisibleFilmstripIds);
  if (filmstripIdsRef.current.join("\0") !== nextVisibleFilmstripIds.join("\0")) {
    filmstripIdsRef.current = nextVisibleFilmstripIds;
  }
  const visibleFilmstripIds = filmstripIdsRef.current;
  useLayoutEffect(() => { filmstripVirtualizer.measure(); }, [filmstripItemSize, filmstripVirtualizer]);
  // Keep the active thumbnail framed through selection moves.
  useLayoutEffect(() => {
    if (activeIndex >= 0) filmstripVirtualizer.scrollToIndex(activeIndex, { align: "auto" });
  }, [active.id, activeIndex, filmstripVirtualizer]);
  useOrientationRetention(thumbnailOrientation, () => {
    // Portrait and landscape filmstrip items have different widths.
    if (activeIndex >= 0) filmstripVirtualizer.scrollToIndex(activeIndex, { align: "auto" });
  });
  useEffect(() => {
    const element = filmstripRef.current;
    return element ? installFilmstripWheel(element) : undefined;
  }, []);
  const preloadAssets = useMemo(() => [
    ...nearbyPreviewAssets,
    ...visibleFilmstripIds.flatMap((id) => assets.find((asset) => asset.id === id) ?? [])
      .filter((asset) => !nearbyPreviewAssets.some((nearby) => nearby.id === asset.id)),
  ], [assets, nearbyPreviewAssets, visibleFilmstripIds]);

  const visibleFilmstripIdSet = useMemo(
    () => new Set(visibleFilmstripIds),
    [visibleFilmstripIds],
  );
  const viewportRankById = useMemo(
    () => new Map(visibleFilmstripIds.map((id, index) => [id, index])),
    [visibleFilmstripIds],
  );
  const visibleFilmstripAssets = useMemo(() => {
    const visibleAssets = new Map(virtualFilmstripItems.flatMap((item) => {
      const asset = assets[item.index];
      return asset ? [[asset.id, asset] as const] : [];
    }));
    return [active, ...visibleFilmstripIds
      .filter((id) => id !== active.id)
      .flatMap((id) => visibleAssets.get(id) ?? [])];
  }, [active, assets, virtualFilmstripItems, visibleFilmstripIds]);
  const visibleMetadataPaths = visibleFilmstripAssets.map((asset) => asset.path);
  const visibleMetadataSignature = visibleMetadataPaths.join("\u0000");
  useEffect(() => {
    if (!visibleMetadataPaths.length) return;
    void requestMetadata(visibleMetadataPaths, "visible")
      .then(acceptMetadataProjections)
      .catch(() => undefined);
  }, [visibleMetadataSignature]);

  const filmstripScheduleIntents = useMemo(() => filmstripPreviewIntents(
    virtualFilmstripItems.flatMap((item) => {
      const asset = assets[item.index];
      return asset ? [{
        asset,
        visible: visibleFilmstripIdSet.has(asset.id),
        distance: Math.abs((item.start + item.end) / 2 - (filmstripViewportStart + filmstripViewportEnd) / 2),
      }] : [];
    }),
    active,
  ), [active, assets, virtualFilmstripItems, visibleFilmstripIdSet, filmstripViewportStart, filmstripViewportEnd]);
  useEffect(() => {
    filmstripSchedule.reconcile(filmstripScheduleIntents);
  }, [filmstripSchedule, filmstripScheduleIntents]);

  useEffect(() => () => {
    filmstripSchedule.release();
  }, [filmstripSchedule]);

  useEffect(() => {
    if (shouldFetchFilmstripPage(
      virtualFilmstripItems.at(-1)?.index,
      assets.length,
      hasNextPage,
      isFetchingNextPage,
    )) {
      fetchNextPage();
    }
  }, [assets.length, fetchNextPage, hasNextPage, isFetchingNextPage, virtualFilmstripItems]);

  return (<>
    <FilmstripPreviewPreloader assets={preloadAssets} />
      <div
        className={`filmstrip filmstrip--${thumbnailOrientation}`}
        ref={filmstripRef}
      >
        <div
          className="filmstrip__track"
          style={{ width: filmstripVirtualizer.getTotalSize() }}
        >
          {virtualFilmstripItems.map((item) => {
            const asset = assets[item.index];
            if (!asset) {
              return (
                <span
                  aria-hidden="true"
                  className="filmstrip__placeholder"
                  key={item.key}
                  style={{
                    transform: `translateX(${item.start}px)`,
                    width: item.size,
                  }}
                />
              );
            }
            return (
              <FilmstripItem
                key={asset.id}
                active={active.id === asset.id}
                asset={asset}
                onSelect={select}
                onContextMenu={onAssetContextMenu}
                rank={viewportRankById.get(asset.id)
                  ?? visibleFilmstripIds.length + Math.abs(item.index - activeIndex)}
                showMetadata={loupeMetadataVisible}
                start={item.start}
                size={item.size}
                visible={visibleFilmstripIdSet.has(asset.id)}
              />
            );
          })}
        </div>
        {isFetchingNextPage ? <span className="filmstrip__loading">Loading...</span> : null}
      </div>
  </>);
});

interface FilmstripItemProps {
  active: boolean;
  asset: AssetSummary;
  onSelect: (id: string) => void;
  onContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
  rank: number;
  showMetadata: boolean;
  start: number;
  size: number;
  visible: boolean;
}

const FilmstripItem = memo(function FilmstripItem({
  active,
  asset,
  onSelect,
  onContextMenu,
  rank,
  showMetadata,
  start,
  size,
  visible,
}: FilmstripItemProps) {
  const onClick = useCallback(() => onSelect(asset.id), [onSelect, asset.id]);
  const handleContextMenu = useCallback((event: React.MouseEvent) => onContextMenu(event, asset), [onContextMenu, asset]);
  const style = useMemo(() => ({ transform: `translateX(${start}px)`, width: size }), [start, size]);
  const tagsQuery = useQuery({
    queryKey: ["asset-tag-assignments", [asset.path]],
    queryFn: ({ signal }) => getBatchedAssetTags(asset.path, signal),
    enabled: visible || active,
    staleTime: Infinity,
  });
  const tags = (tagsQuery.data ?? []).filter((assignment) => assignment.assignedCount > 0);
  return (
    <button
      data-filmstrip-asset-id={asset.id}
      className={active ? "is-active" : ""}
      onClick={onClick}
      onContextMenu={handleContextMenu}
      style={style}
      title={asset.name}
    >
      <Thumbnail
        asset={asset}
        enabled
        priority={active ? "loupe" : visible ? "visible" : "nearby"}
        rank={rank}
      />
      {showMetadata || tags.length > 0 ? (
        <span className="filmstrip__metadata">
          {showMetadata ? <AssetMetadataBadges asset={asset} /> : null}
          <span className="filmstrip__tags" title={tags.map(({ tag }) => tag.path.replaceAll("|", " › ")).join("\n")}>
            {tags.map(({ tag }) => <span key={tag.id}>{tag.name}</span>)}
          </span>
        </span>
      ) : null}
      <span className="filmstrip__name">{asset.name}</span>
    </button>
  );
});
