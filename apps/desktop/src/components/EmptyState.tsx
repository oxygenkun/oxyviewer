import { Aperture, ArrowUpRight, DatabaseZap, FolderOpen, Gauge } from "lucide-react";
import type { MessageKey } from "../lib/i18n";

interface EmptyStateProps {
  onOpen: () => void;
  t: (key: MessageKey) => string;
}

export function EmptyState({ onOpen, t }: EmptyStateProps) {
  return (
    <main className="welcome">
      <div className="welcome__orb welcome__orb--one" />
      <div className="welcome__orb welcome__orb--two" />
      <section className="welcome__content">
        <div className="welcome__mark">
          <Aperture size={30} strokeWidth={1.5} />
          <span>OXY / 01</span>
        </div>
        <p className="eyebrow">LOCAL-FIRST PHOTO DESK</p>
        <h1>{t("emptyTitle")}</h1>
        <p className="welcome__lead">{t("emptyBody")}</p>
        <button className="primary-action primary-action--large" onClick={onOpen}>
          <FolderOpen size={18} />
          {t("openFolder")}
          <ArrowUpRight size={17} />
        </button>
        <div className="welcome__features">
          <span><Gauge size={15} /> {t("directBrowse")}</span>
          <span><DatabaseZap size={15} /> {t("transparentCache")}</span>
          <span><Aperture size={15} /> {t("rawReady")}</span>
        </div>
      </section>
      <aside className="welcome__index" aria-hidden="true">
        <span>ARW</span><span>CR3</span><span>NEF</span><span>HEIC</span><span>JPEG</span>
      </aside>
    </main>
  );
}

