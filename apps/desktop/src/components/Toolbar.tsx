import {
  ArrowDownAZ,
  ArrowDownUp,
  Columns3,
  Grid3X3,
  List,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Palette,
  Search,
  Star,
  SlidersHorizontal,
  X,
} from "lucide-react";
import type { MessageKey } from "../lib/i18n";
import { useWorkspaceStore } from "../store";
import type { AssetKind, AssetSort, ViewMode } from "../types";

const kinds: Array<AssetKind | undefined> = [undefined, "raw", "jpeg", "heif"];
const views: Array<[ViewMode, typeof Grid3X3, MessageKey]> = [
  ["grid", Grid3X3, "viewGrid"],
  ["list", List, "viewList"],
  ["loupe", Columns3, "viewLoupe"],
];

interface ToolbarProps {
  total: number;
  t: (key: MessageKey) => string;
}

export function Toolbar({ total, t }: ToolbarProps) {
  const {
    view, setView, search, setSearch, kind, setKind, sort, setSort, direction,
    toggleDirection, inspectorOpen, toggleInspector, leftPanelOpen, toggleLeftPanel,
    minimumRating, setMinimumRating, colorLabel, setColorLabel,
  } = useWorkspaceStore();

  return (
    <>
      <header className="toolbar">
        <button className="icon-button" title={t("collapse")} onClick={toggleLeftPanel}>
          {leftPanelOpen ? <PanelLeftClose size={16} /> : <PanelLeftOpen size={16} />}
        </button>
        <div className="toolbar__title">
          <strong>WORKSPACE</strong>
          <span>{total.toLocaleString()} {t("photos")}</span>
        </div>
        <label className="search-field">
          <Search size={15} />
          <input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t("search")}
          />
          {search ? <button onClick={() => setSearch("")}><X size={13} /></button> : null}
          <kbd>⌘ K</kbd>
        </label>
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
        <div className="filterbar__sort">
          <Star size={13} />
          <select
            value={minimumRating ?? ""}
            onChange={(event) => setMinimumRating(event.target.value ? Number(event.target.value) : undefined)}
            aria-label={t("ratingFilter")}
          >
            <option value="">{t("anyRating")}</option>
            {[1, 2, 3, 4, 5].map((rating) => (
              <option key={rating} value={rating}>{rating}★+</option>
            ))}
          </select>
          <Palette size={13} />
          <select
            value={colorLabel ?? ""}
            onChange={(event) => setColorLabel(event.target.value || undefined)}
            aria-label={t("colorFilter")}
          >
            <option value="">{t("anyColor")}</option>
            {["Red", "Yellow", "Green", "Blue", "Purple"].map((label) => (
              <option key={label} value={label}>{t(label.toLowerCase() as MessageKey)}</option>
            ))}
          </select>
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
    </>
  );
}

