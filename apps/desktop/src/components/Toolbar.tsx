import {
  ArrowDownAZ,
  ArrowDownUp,
  Columns3,
  Grid3X3,
  Info,
  Languages,
  List,
  PanelLeftClose,
  PanelLeftOpen,
  Search,
  SlidersHorizontal,
  X,
} from "lucide-react";
import type { MessageKey } from "../lib/i18n";
import { useWorkspaceStore } from "../store";
import type { AssetKind, AssetSort, ViewMode } from "../types";

const kinds: Array<AssetKind | undefined> = [undefined, "raw", "jpeg", "heif"];
const views: Array<[ViewMode, typeof Grid3X3]> = [
  ["grid", Grid3X3],
  ["list", List],
  ["loupe", Columns3],
];

interface ToolbarProps {
  total: number;
  t: (key: MessageKey) => string;
}

export function Toolbar({ total, t }: ToolbarProps) {
  const {
    view, setView, search, setSearch, kind, setKind, sort, setSort, direction,
    toggleDirection, inspectorOpen, toggleInspector, leftPanelOpen, toggleLeftPanel,
    locale, setLocale,
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
        <div className="segmented" aria-label="View mode">
          {views.map(([mode, Icon]) => (
            <button
              key={mode}
              className={view === mode ? "is-active" : ""}
              onClick={() => setView(mode)}
              title={mode}
            >
              <Icon size={15} />
            </button>
          ))}
        </div>
        <button className="icon-button" onClick={toggleInspector} title={t("inspector")}>
          <Info size={16} className={inspectorOpen ? "accent-icon" : ""} />
        </button>
        <button
          className="icon-button"
          onClick={() => setLocale(locale === "zh-CN" ? "en" : "zh-CN")}
          title="中文 / English"
        >
          <Languages size={16} />
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

