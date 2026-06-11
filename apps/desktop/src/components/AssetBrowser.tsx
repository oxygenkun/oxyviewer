import { useVirtualizer } from "@tanstack/react-virtual";
import { FileImage } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";
import type { MessageKey } from "../lib/i18n";
import { useWorkspaceStore } from "../store";
import type { AssetSummary, ViewMode } from "../types";
import { Loupe } from "./Loupe";
import { Thumbnail } from "./Thumbnail";

interface AssetBrowserProps {
  assets: AssetSummary[];
  total: number;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => void;
  view: ViewMode;
  t: (key: MessageKey) => string;
}

interface AssetCardProps {
  asset: AssetSummary;
  selected: boolean;
  onSelect: (event: React.MouseEvent) => void;
  onOpen: () => void;
}

const AssetCard = memo(function AssetCard({ asset, selected, onSelect, onOpen }: AssetCardProps) {
  return (
    <button
      className={`asset-card ${selected ? "is-selected" : ""}`}
      onClick={onSelect}
      onDoubleClick={onOpen}
      title={asset.path}
    >
      <Thumbnail asset={asset} />
      <span className="asset-card__name">{asset.name}</span>
      <span className="asset-card__meta">
        {asset.extension}
        {asset.hasSidecar ? <i title="XMP sidecar" /> : null}
      </span>
    </button>
  );
});

export function AssetBrowser(props: AssetBrowserProps) {
  if (props.assets.length === 0) {
    return (
      <div className="no-results">
        <FileImage size={31} strokeWidth={1.25} />
        <strong>{props.t("noResults")}</strong>
        <span>{props.t("noResultsBody")}</span>
      </div>
    );
  }
  if (props.view === "loupe") return <Loupe assets={props.assets} t={props.t} />;
  if (props.view === "list") return <VirtualList {...props} />;
  return <VirtualGrid {...props} />;
}

function VirtualGrid({
  assets,
  hasNextPage,
  isFetchingNextPage,
  fetchNextPage,
}: AssetBrowserProps) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(900);
  const selectedIds = useWorkspaceStore((state) => state.selectedIds);
  const select = useWorkspaceStore((state) => state.select);
  const setView = useWorkspaceStore((state) => state.setView);
  const columns = Math.max(2, Math.floor(width / 190));
  const rowCount = Math.ceil(assets.length / columns);
  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 194,
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
    const last = rows.at(-1);
    if (last && last.index >= rowCount - 2 && hasNextPage && !isFetchingNextPage) fetchNextPage();
  }, [fetchNextPage, hasNextPage, isFetchingNextPage, rowCount, rows]);

  return (
    <div className="asset-scroll" ref={parentRef}>
      <div className="virtual-grid" style={{ height: virtualizer.getTotalSize() }}>
        {rows.map((row) => (
          <div
            className="virtual-grid__row"
            key={row.key}
            style={{
              gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
              transform: `translateY(${row.start}px)`,
            }}
          >
            {assets.slice(row.index * columns, row.index * columns + columns).map((asset) => (
              <AssetCard
                key={asset.id}
                asset={asset}
                selected={selectedIds.includes(asset.id)}
                onSelect={(event) => select(asset.id, event.metaKey || event.ctrlKey)}
                onOpen={() => {
                  select(asset.id);
                  setView("loupe");
                }}
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
}: AssetBrowserProps) {
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
        <span>Name</span><span>Type</span><span>Size</span><span>Modified</span>
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
              <Thumbnail asset={asset} />
              <strong>{asset.name}</strong>
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
