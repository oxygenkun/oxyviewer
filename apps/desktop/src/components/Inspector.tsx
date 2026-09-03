import { useMutation, useQuery, useQueryClient, type InfiniteData } from "@tanstack/react-query";
import { useState } from "react";
import { Circle, Download, FileCog, FolderSearch, Image, Star, Tag, X } from "lucide-react";
import {
  chooseAndConfigureExiftool,
  getAssetDetails,
  getExiftoolStatus,
  installExiftool,
  patchMetadata,
  syncMetadataToEmbedded,
} from "../lib/api";
import type { MessageKey } from "../lib/i18n";
import { patchAssetDetails, patchAssetPages, patchAssetSummaries } from "../lib/metadataCache";
import { requiresExiftoolSetup } from "../lib/metadataProvider";
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
  const [syncPending, setSyncPending] = useState(false);
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
  const metadataPaths = selectedPaths.length ? selectedPaths : asset ? [asset.path] : [];
  const syncEmbedded = useMutation({
    mutationFn: () => syncMetadataToEmbedded(metadataPaths),
    onSettled: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["asset-details"] }),
        queryClient.invalidateQueries({ queryKey: ["assets"] }),
        queryClient.invalidateQueries({ queryKey: ["asset-metadata"] }),
      ]);
    },
  });
  const providerSetup = useMutation({
    mutationFn: (source: "download" | "select") => source === "download"
      ? installExiftool()
      : chooseAndConfigureExiftool(),
    onSuccess: async (status) => {
      if (!status?.available || !syncPending) return;
      setSyncPending(false);
      await queryClient.invalidateQueries({ queryKey: ["asset-details", asset?.id] });
      syncEmbedded.mutate();
    },
  });
  const providerCheck = useMutation({
    mutationFn: getExiftoolStatus,
    onSuccess: (status) => {
      if (asset && requiresExiftoolSetup(asset.kind, !status.available)) {
        setSyncPending(true);
        providerSetup.reset();
      } else {
        syncEmbedded.mutate();
      }
    },
  });
  const requestPatch = (value: MetadataPatch) => {
    patch.mutate(value);
  };
  const requestEmbeddedSync = () => {
    providerCheck.mutate();
  };
  const availableColorLabels = asset?.extension.toLowerCase() === "hif"
    ? sonyHifColorLabels
    : colorLabels;
  const capture = details.data?.captureMetadata;

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
          <InspectorSection title={t("basicInfo")}>
            <div className="capture-strip">
              <CaptureValue label={t("aperture")} value={capture?.aperture} />
              <CaptureValue label={t("shutterSpeed")} value={capture?.exposureTime} />
              <CaptureValue label={t("focalLength")} value={capture?.focalLength} />
              <CaptureValue label={t("iso")} value={capture?.iso ? `ISO ${capture.iso}` : undefined} />
            </div>
            <DataRow label={t("exposureCompensation")} value={capture?.exposureCompensation ?? "—"} />
            <DataRow label={t("pixelDimensions")} value={
              details.data?.width ? `${details.data.width} × ${details.data.height}` : "—"
            } />
            <DataRow label={t("chromaSubsampling")} value={capture?.chromaSubsampling ?? "—"} />
            <DataRow label={t("capturedAt")} value={formatExifDate(capture?.capturedAt)} />
            <DataRow label={t("modified")} value={new Date(asset.modifiedAtMs).toLocaleString()} />
            <DataRow label={t("cameraMake")} value={capture?.cameraMake ?? "—"} />
            <DataRow label={t("cameraModel")} value={capture?.cameraModel ?? "—"} />
            <DataRow label={t("lensMake")} value={capture?.lensMake ?? "—"} />
            <DataRow label={t("lensModel")} value={capture?.lensModel ?? "—"} />
          </InspectorSection>
          <InspectorSection title={t("metadata")}>
            <label>{t("rating")}</label>
            <div className="rating" aria-label={t("rating")}>
              {[1, 2, 3, 4, 5].map((rating) => (
                <button
                  key={rating}
                  onClick={() => requestPatch({ rating: currentRating === rating ? null : rating })}
                  disabled={patch.isPending || details.isLoading || providerSetup.isPending}
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
                  onClick={() => requestPatch({
                    colorLabel: currentColor?.toLowerCase() === label.toLowerCase() ? null : label,
                  })}
                  disabled={patch.isPending || details.isLoading || providerSetup.isPending}
                  title={t(label.toLowerCase() as MessageKey)}
                />
              ))}
              <button
                className="color-labels__clear"
                onClick={() => requestPatch({ colorLabel: null })}
                disabled={patch.isPending || !currentColor || providerSetup.isPending}
                title={t("clearColor")}
              ><X size={11} /></button>
            </div>
            {patch.isError || details.isError ? (
              <small className="metadata-error">{String(patch.error ?? details.error)}</small>
            ) : null}
            {asset.kind !== "raw" && details.data?.asset.hasSidecar ? (
              <button
                className="metadata-sync-button"
                onClick={requestEmbeddedSync}
                disabled={syncEmbedded.isPending || providerSetup.isPending || providerCheck.isPending}
              >
                {syncEmbedded.isPending ? t("syncingEmbeddedMetadata") : t("syncEmbeddedMetadata")}
              </button>
            ) : null}
            {syncEmbedded.isError ? (
              <small className="metadata-error">{String(syncEmbedded.error)}</small>
            ) : null}
            {providerCheck.isError ? (
              <small className="metadata-error">{String(providerCheck.error)}</small>
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
            <DataRow label={t("size")} value={formatBytes(asset.sizeBytes)} />
            <DataRow
              label={t("sidecar")}
              value={details.data?.asset.hasSidecar ? "XMP" : "—"}
              accent={details.data?.asset.hasSidecar}
            />
            <DataRow
              label={t("metadataSource")}
              value={details.data?.asset.hasSidecar
                ? t("sidecarOverridesEmbedded")
                : details.data?.metadataCapability.provider === "native" ||
                    details.data?.metadataCapability.provider === "exiftool"
                  ? t("embeddedMetadata")
                  : "—"}
            />
          </InspectorSection>
        </div>
      )}
      {syncPending ? (
        <div className="metadata-provider-overlay" role="presentation">
          <div
            className="metadata-provider-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="exiftool-required-title"
          >
            <header>
              <strong id="exiftool-required-title">{t("exiftoolRequiredTitle")}</strong>
              <button
                onClick={() => setSyncPending(false)}
                disabled={providerSetup.isPending}
                aria-label={t("cancel")}
              ><X size={14} /></button>
            </header>
            <p>{t("exiftoolRequiredBody")}</p>
            {providerSetup.isPending ? <small>{t("installingExiftool")}</small> : null}
            {providerSetup.isError ? (
              <small className="metadata-error">{String(providerSetup.error)}</small>
            ) : null}
            <div className="metadata-provider-dialog__actions">
              <button
                className="is-primary"
                onClick={() => providerSetup.mutate("download")}
                disabled={providerSetup.isPending}
              ><Download size={13} />{t("downloadExiftool")}</button>
              <button
                onClick={() => providerSetup.mutate("select")}
                disabled={providerSetup.isPending}
              ><FolderSearch size={13} />{t("selectExiftool")}</button>
              <button
                onClick={() => setSyncPending(false)}
                disabled={providerSetup.isPending}
              >{t("cancel")}</button>
            </div>
          </div>
        </div>
      ) : null}
    </aside>
  );
}

function CaptureValue({ label, value }: { label: string; value?: string }) {
  return (
    <div className="capture-value">
      <strong>{value ?? "—"}</strong>
      <span>{label}</span>
    </div>
  );
}

function formatExifDate(value?: string): string {
  if (!value) return "—";
  return value.replace(/^(\d{4}):(\d{2}):(\d{2})/, "$1-$2-$3");
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
