import { useEffect, useState } from "react";
import {
  ArrowDownAZ,
  ArrowDownUp,
  Check,
  Grid3x2,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Search,
  SlidersHorizontal,
  Star,
} from "lucide-react";
import type { MessageKey } from "@/lib/i18n";
import { useWorkspaceStore } from "@/store";
import type { AssetKind, AssetSort, PickLabel, ViewMode } from "@/types";
import { WorkspaceSearch } from "./WorkspaceSearch";
import { PickFlagIcon } from "@/components/common/PickFlagIcon";

const kinds: Array<AssetKind | undefined> = [undefined, "raw", "jpeg", "heif"];
const ratings = [1, 2, 3, 4, 5] as const;
const colorLabels = ["Red", "Yellow", "Green", "Blue", "Purple"] as const;
const pickLabels: Array<[PickLabel, MessageKey]> = [
  ["accepted", "flagAccepted"],
  ["pending", "flagPending"],
  ["rejected", "flagRejected"],
];
const views: Array<[ViewMode, typeof Grid3x2, MessageKey]> = [
  ["grid", Grid3x2, "viewGrid"],
  ["loupe", Search, "viewLoupe"],
];

interface ToolbarProps {
  total: number;
  loading?: boolean;
  t: (key: MessageKey) => string;
}

export function Toolbar({ total, loading, t }: ToolbarProps) {
  const {
    view, setView, kind, setKind, sort, setSort, direction,
    toggleDirection, inspectorOpen, toggleInspector, leftPanelOpen, toggleLeftPanel,
    minimumRating, setMinimumRating, colorLabels: selectedColorLabels, toggleColorLabel,
    pickLabels: selectedPickLabels, togglePickLabel,
  } = useWorkspaceStore();

  const [announcedTotal, setAnnouncedTotal] = useState(total);
  useEffect(() => {
    if (loading) return;
    const timer = window.setTimeout(() => setAnnouncedTotal(total), 250);
    return () => window.clearTimeout(timer);
  }, [total, loading]);

  return (
    <>
      <header className="toolbar">
        <button className="icon-button" title={t("collapse")} onClick={toggleLeftPanel}>
          {leftPanelOpen ? <PanelLeftClose size={16} /> : <PanelLeftOpen size={16} />}
        </button>
        <div className="toolbar__title">
          <strong>WORKSPACE</strong>
          <span>{loading ? t("loading") : `${total.toLocaleString()} ${t("photos")}`}</span>
        </div>
        <span className="search-result-announcement" aria-live="polite" aria-atomic="true">{announcedTotal.toLocaleString()} {t("photos")}</span>
        <WorkspaceSearch t={t} />
        <div className="segmented" aria-label={t("viewMode")}>
          {views.map(([mode, Icon, label]) => (
            <button
              key={mode}
              className={view === mode ? "is-active" : ""}
              onClick={() => setView(mode)}
              title={t(label)}
              aria-label={t(label)}
            >
              <Icon size={15} />
            </button>
          ))}
        </div>
        <button className="icon-button" onClick={toggleInspector} title={t("inspector")}>
          {inspectorOpen ? <PanelRightClose size={16} /> : <PanelRightOpen size={16} />}
        </button>
      </header>
      <div className="filterbar">
        <div className="filterbar__kinds">
          {kinds.map((value) => (
            <button
              key={value ?? "all"}
              className={kind === value ? "is-active" : ""}
              onClick={() => setKind(value)}
            >
              {value?.toUpperCase() ?? t("all")}
            </button>
          ))}
        </div>
        <div className="filterbar__controls">
          <div className="filterbar__metadata-filters">
            <div className="filterbar__rating" role="group" aria-label={t("ratingFilter")}>
              {ratings.map((rating) => {
                const active = (minimumRating ?? 0) >= rating;
                return (
                  <button
                    key={rating}
                    className={active ? "is-active" : ""}
                    onClick={() => setMinimumRating(minimumRating === rating ? undefined : rating)}
                    aria-label={`${rating}★+`}
                    aria-pressed={active}
                    title={`${t("ratingFilter")}: ${rating}★+`}
                  >
                    <Star size={13} />
                  </button>
                );
              })}
            </div>
            <div className="filterbar__colors" role="group" aria-label={t("colorFilter")}>
              {colorLabels.map((label) => {
                const active = selectedColorLabels.includes(label);
                return (
                  <button
                    key={label}
                    className={active ? "is-active" : ""}
                    style={{ "--filter-color": `var(--label-${label.toLowerCase()})` } as React.CSSProperties}
                    onClick={() => toggleColorLabel(label)}
                    aria-label={t(label.toLowerCase() as MessageKey)}
                    aria-pressed={active}
                    title={t(label.toLowerCase() as MessageKey)}
                  >
                    {active ? <Check size={9} strokeWidth={3} /> : null}
                  </button>
                );
              })}
            </div>
            <div className="filterbar__flags" role="group" aria-label={t("flagFilter")}>
              {pickLabels.map(([value, label]) => {
                const active = selectedPickLabels.includes(value);
                return (
                  <button
                    key={value}
                    className={`filterbar__flag filterbar__flag--${value}${active ? " is-active" : ""}`}
                    onClick={() => togglePickLabel(value)}
                    aria-label={t(label)}
                    aria-pressed={active}
                    title={`${t("flagFilter")}: ${t(label)}`}
                  >
                    <PickFlagIcon value={value} size={13} />
                  </button>
                );
              })}
            </div>
          </div>
          <div className="filterbar__sort">
            <SlidersHorizontal size={14} />
            <select value={sort} onChange={(event) => setSort(event.target.value as AssetSort)}>
              <option value="name">{t("sortName")}</option>
              <option value="modified">{t("sortModified")}</option>
              <option value="size">{t("sortSize")}</option>
            </select>
            <button onClick={toggleDirection} title={direction}>
              {sort === "name" ? <ArrowDownAZ size={15} /> : <ArrowDownUp size={15} />}
            </button>
          </div>
        </div>
      </div>
    </>
  );
}

