import { create } from "zustand";
import type { Locale } from "./lib/i18n";
import {
  loadFocusAreasVisible,
  loadLayoutSize,
  loadLoupeControlsAutoHide,
  loadMetadataVisibility,
  loadUiFontScale,
  saveFocusAreasVisible,
  saveLayoutSize,
  saveLoupeControlsAutoHide,
  saveMetadataVisibility,
  saveUiFontScale,
  type UiFontScale,
} from "./lib/workspacePersistence";
import type {
  AssetKind,
  AssetSort,
  GridPreference,
  NavigatorPosition,
  SortDirection,
  ViewMode,
} from "./types";

interface WorkspaceState {
  view: ViewMode;
  gridPreference: GridPreference;
  selectedIds: string[];
  activeId?: string;
  inspectorOpen: boolean;
  leftPanelOpen: boolean;
  settingsOpen: boolean;
  locale: Locale;
  navigatorVisible: boolean;
  navigatorPosition: NavigatorPosition;
  hardwareAcceleration: boolean;
  displaySharpening: boolean;
  focusAreasVisible: boolean;
  gridMetadataVisible: boolean;
  loupeMetadataVisible: boolean;
  loupeControlsAutoHide: boolean;
  uiFontScale: UiFontScale;
  leftPanelWidth: number;
  inspectorWidth: number;
  filmstripHeight: number;
  search: string;
  kind?: AssetKind;
  minimumRating?: number;
  colorLabels: string[];
  sort: AssetSort;
  direction: SortDirection;
  setView: (view: ViewMode) => void;
  setGridPreference: (preference: GridPreference) => void;
  select: (id: string, additive?: boolean) => void;
  clearSelection: () => void;
  toggleInspector: () => void;
  toggleLeftPanel: () => void;
  toggleSettings: () => void;
  setLocale: (locale: Locale) => void;
  setNavigatorVisible: (visible: boolean) => void;
  setNavigatorPosition: (position: NavigatorPosition) => void;
  setHardwareAcceleration: (enabled: boolean) => void;
  setDisplaySharpening: (enabled: boolean) => void;
  setFocusAreasVisible: (visible: boolean) => void;
  setGridMetadataVisible: (visible: boolean) => void;
  setLoupeMetadataVisible: (visible: boolean) => void;
  setLoupeControlsAutoHide: (enabled: boolean) => void;
  setUiFontScale: (scale: UiFontScale) => void;
  setLeftPanelWidth: (width: number) => void;
  setInspectorWidth: (width: number) => void;
  setFilmstripHeight: (height: number) => void;
  setSearch: (search: string) => void;
  setKind: (kind?: AssetKind) => void;
  setMinimumRating: (rating?: number) => void;
  toggleColorLabel: (label: string) => void;
  setSort: (sort: AssetSort) => void;
  toggleDirection: () => void;
}

export const useWorkspaceStore = create<WorkspaceState>((set) => ({
  view: "grid",
  gridPreference: "landscape",
  selectedIds: [],
  inspectorOpen: true,
  leftPanelOpen: true,
  settingsOpen: false,
  locale: "zh-CN",
  navigatorVisible: true,
  navigatorPosition: "bottom-right",
  hardwareAcceleration: true,
  displaySharpening: true,
  focusAreasVisible: loadFocusAreasVisible(),
  gridMetadataVisible: loadMetadataVisibility("grid"),
  loupeMetadataVisible: loadMetadataVisibility("loupe"),
  loupeControlsAutoHide: loadLoupeControlsAutoHide(),
  uiFontScale: loadUiFontScale(),
  leftPanelWidth: loadLayoutSize("leftPanel"),
  inspectorWidth: loadLayoutSize("inspector"),
  filmstripHeight: loadLayoutSize("filmstrip"),
  search: "",
  colorLabels: [],
  sort: "name",
  direction: "ascending",
  setView: (view) => set({ view }),
  setGridPreference: (gridPreference) => set({ gridPreference }),
  select: (id, additive = false) =>
    set((state) => {
      if (!additive) return { selectedIds: [id], activeId: id };
      const selectedIds = state.selectedIds.includes(id)
        ? state.selectedIds.filter((selected) => selected !== id)
        : [...state.selectedIds, id];
      return { selectedIds, activeId: id };
    }),
  clearSelection: () => set({ selectedIds: [], activeId: undefined }),
  toggleInspector: () => set((state) => ({ inspectorOpen: !state.inspectorOpen })),
  toggleLeftPanel: () => set((state) => ({ leftPanelOpen: !state.leftPanelOpen })),
  toggleSettings: () => set((state) => ({ settingsOpen: !state.settingsOpen })),
  setLocale: (locale) => set({ locale }),
  setNavigatorVisible: (navigatorVisible) => set({ navigatorVisible }),
  setNavigatorPosition: (navigatorPosition) => set({ navigatorPosition }),
  setHardwareAcceleration: (hardwareAcceleration) => set({ hardwareAcceleration }),
  setDisplaySharpening: (displaySharpening) => set({ displaySharpening }),
  setFocusAreasVisible: (focusAreasVisible) => {
    saveFocusAreasVisible(focusAreasVisible);
    set({ focusAreasVisible });
  },
  setGridMetadataVisible: (gridMetadataVisible) => {
    saveMetadataVisibility("grid", gridMetadataVisible);
    set({ gridMetadataVisible });
  },
  setLoupeMetadataVisible: (loupeMetadataVisible) => {
    saveMetadataVisibility("loupe", loupeMetadataVisible);
    set({ loupeMetadataVisible });
  },
  setLoupeControlsAutoHide: (loupeControlsAutoHide) => {
    saveLoupeControlsAutoHide(loupeControlsAutoHide);
    set({ loupeControlsAutoHide });
  },
  setUiFontScale: (uiFontScale) => {
    saveUiFontScale(uiFontScale);
    set({ uiFontScale });
  },
  setLeftPanelWidth: (leftPanelWidth) => {
    saveLayoutSize("leftPanel", leftPanelWidth);
    set({ leftPanelWidth });
  },
  setInspectorWidth: (inspectorWidth) => {
    saveLayoutSize("inspector", inspectorWidth);
    set({ inspectorWidth });
  },
  setFilmstripHeight: (filmstripHeight) => {
    saveLayoutSize("filmstrip", filmstripHeight);
    set({ filmstripHeight });
  },
  setSearch: (search) => set({ search }),
  setKind: (kind) => set({ kind }),
  setMinimumRating: (minimumRating) => set({ minimumRating }),
  toggleColorLabel: (label) => set((state) => ({
    colorLabels: state.colorLabels.includes(label)
      ? state.colorLabels.filter((colorLabel) => colorLabel !== label)
      : [...state.colorLabels, label],
  })),
  setSort: (sort) => set({ sort }),
  toggleDirection: () =>
    set((state) => ({
      direction: state.direction === "ascending" ? "descending" : "ascending",
    })),
}));
