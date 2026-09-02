import { useMutation, useQuery, useQueryClient, type InfiniteData } from "@tanstack/react-query";
import { Circle, FileCog, Image, Star, Tag, X } from "lucide-react";
import { getAssetDetails, patchMetadata } from "../lib/api";
import type { MessageKey } from "../lib/i18n";
import { patchAssetDetails, patchAssetPages, patchAssetSummaries } from "../lib/metadataCache";
import type { AssetDetails, AssetSummary, MetadataPatch, Page } from "../types";
import { formatBytes } from "./AssetBrowser";
import { Thumbnail } from "./Thumbnail";

interface InspectorProps {
  asset?: AssetSummary;
  selectedCount: number;
  selectedPaths: string[];
  t: (key: MessageKey) => string;
}

const colorLabels = ["Red", "Yellow", "Green", "Blue", "Purple"] as const;
const sonyHifColorLabels = colorLabels.filter((label) => label !== "Purple");

export function Inspector({ asset, selectedCount, selectedPaths, t }: InspectorProps) {
  const queryClient = useQueryClient();
  const details = useQuery({
    queryKey: ["asset-details", asset?.id],
    queryFn: () => getAssetDetails(asset!),
    enabled: Boolean(asset),
  });
  const patch = useMutation({
    mutationFn: (value: MetadataPatch) =>
      patchMetadata(selectedPaths.length ? selectedPaths : [asset!.path], value),
    onMutate: async (value) => {
      const paths = new Set(selectedPaths.length ? selectedPaths : [asset!.path]);
      const filters = [
        { queryKey: ["asset-details"] },
        { queryKey: ["assets"] },
        { queryKey: ["asset-metadata"] },
        { queryKey: ["preload-assets"] },
      ];
      await Promise.all(filters.map((filter) => queryClient.cancelQueries(filter)));
      const snapshots = filters.flatMap((filter) => queryClient.getQueriesData(filter));

      queryClient.setQueriesData<AssetDetails>({ queryKey: ["asset-details"] }, (current) =>
        patchAssetDetails(current, paths, value));
      queryClient.setQueriesData<AssetSummary[]>({ queryKey: ["asset-metadata"] }, (current) =>
        patchAssetSummaries(current, paths, value));
      for (const queryKey of [["assets"], ["preload-assets"]] as const) {
        queryClient.setQueriesData<InfiniteData<Page<AssetSummary>>>({ queryKey }, (current) =>
          patchAssetPages(current, paths, value));
      }
      return { snapshots };
    },
    onError: (_error, _value, context) => {
      for (const [queryKey, data] of context?.snapshots ?? []) {
        queryClient.setQueryData(queryKey, data);
      }
    },
    onSettled: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["asset-details"] }),
        queryClient.invalidateQueries({ queryKey: ["assets"] }),
        queryClient.invalidateQueries({ queryKey: ["asset-metadata"] }),
        queryClient.invalidateQueries({ queryKey: ["preload-assets"] }),
      ]);
    },
  });
  const currentRating = details.data?.metadata.rating;
  const currentColor = details.data?.metadata.colorLabel;
  const availableColorLabels = asset?.extension.toLowerCase() === "hif"
    ? sonyHifColorLabels
    : colorLabels;

  return (
    <aside className="inspector">
      <header className="inspector__header">
        <div><strong>{t("inspector")}</strong><span>{selectedCount || "—"} {t("selected")}</span></div>
        <FileCog size={16} />
      </header>
      {!asset ? (
        <div className="inspector__empty">
          <Image size={25} strokeWidth={1.25} />
          <span>{t("noSelection")}</span>
        </div>
      ) : (
        <div className="inspector__body">
          <Thumbnail asset={asset} />
          <div className="inspector__asset-title">
            <strong>{asset.name}</strong>
            <span>{asset.path}</span>
          </div>
          <InspectorSection title={t("metadata")}>
            <label>{t("rating")}</label>
            <div className="rating" aria-label={t("rating")}>
              {[1, 2, 3, 4, 5].map((rating) => (
                <button
                  key={rating}
                  onClick={() => patch.mutate({ rating: currentRating === rating ? null : rating })}
                  disabled={patch.isPending || details.isLoading}
                  title={`${rating} / 5`}
                >
                  <Star
                    className={(currentRating ?? 0) >= rating ? "is-filled" : undefined}
                    size={15}
                  />
                </button>
              ))}
            </div>
            <label>{t("colorLabel")}</label>
            <div className="color-labels" aria-label={t("colorLabel")}>
              {availableColorLabels.map((label) => (
                <button
                  key={label}
                  className={currentColor?.toLowerCase() === label.toLowerCase() ? "is-active" : ""}
                  style={{ "--label-color": `var(--label-${label.toLowerCase()})` } as React.CSSProperties}
                  onClick={() => patch.mutate({
                    colorLabel: currentColor?.toLowerCase() === label.toLowerCase() ? null : label,
                  })}
                  disabled={patch.isPending || details.isLoading}
                  title={t(label.toLowerCase() as MessageKey)}
                />
              ))}
              <button
                className="color-labels__clear"
                onClick={() => patch.mutate({ colorLabel: null })}
                disabled={patch.isPending || !currentColor}
                title={t("clearColor")}
              ><X size={11} /></button>
            </div>
            {patch.isError || details.isError ? (
              <small className="metadata-error">{String(patch.error ?? details.error)}</small>
            ) : null}
            <label>{t("keywords")}</label>
            <div className="tags">
              {(details.data?.metadata.keywords ?? []).map((keyword) => (
                <span key={keyword}><Tag size={10} />{keyword}</span>
              ))}
              {(details.data?.metadata.keywords.length ?? 0) === 0 ? <em>—</em> : null}
            </div>
          </InspectorSection>
          <InspectorSection title={t("fileInfo")}>
            <DataRow label={t("format")} value={asset.extension} />
            <DataRow label={t("dimensions")} value={
              details.data?.width ? `${details.data.width} × ${details.data.height}` : "—"
            } />
            <DataRow label={t("size")} value={formatBytes(asset.sizeBytes)} />
            <DataRow label={t("modified")} value={new Date(asset.modifiedAtMs).toLocaleString()} />
            <DataRow label={t("sidecar")} value={asset.hasSidecar ? "XMP" : "—"} accent={asset.hasSidecar} />
          </InspectorSection>
        </div>
      )}
    </aside>
  );
}

function InspectorSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="inspector-section">
      <h3>{title}</h3>
      {children}
    </section>
  );
}

function DataRow({ label, value, accent = false }: { label: string; value: string; accent?: boolean }) {
  return (
    <div className="data-row">
      <span>{label}</span>
      <strong className={accent ? "data-row--accent" : ""}>
        {accent ? <Circle size={6} fill="currentColor" /> : null}{value}
      </strong>
    </div>
  );
}

