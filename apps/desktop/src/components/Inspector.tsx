import { useQuery } from "@tanstack/react-query";
import { Circle, FileCog, Image, Star, Tag } from "lucide-react";
import { getAssetDetails } from "../lib/api";
import type { MessageKey } from "../lib/i18n";
import type { AssetSummary } from "../types";
import { formatBytes } from "./AssetBrowser";
import { Thumbnail } from "./Thumbnail";

interface InspectorProps {
  asset?: AssetSummary;
  selectedCount: number;
  t: (key: MessageKey) => string;
}

export function Inspector({ asset, selectedCount, t }: InspectorProps) {
  const details = useQuery({
    queryKey: ["asset-details", asset?.id],
    queryFn: () => getAssetDetails(asset!),
    enabled: Boolean(asset),
  });

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
            <div className="rating">
              {[1, 2, 3, 4, 5].map((rating) => (
                <Star
                  key={rating}
                  size={15}
                  fill={(details.data?.metadata.rating ?? 0) >= rating ? "currentColor" : "none"}
                />
              ))}
            </div>
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

