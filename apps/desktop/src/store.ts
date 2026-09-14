import { create } from "zustand";
import type { Locale } from "./lib/i18n";
import {
  DEFAULT_SHORTCUTS,
  loadShortcuts,
  saveShortcuts,
  type ShortcutAction,
  type ShortcutBinding,
  type ShortcutBindings,
} from "./lib/shortcuts";
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
  NavigatorPosition,
  SortDirection,
  ThumbnailOrientation,
  ViewMode,
} from "./types";

interface WorkspaceState {
  view: ViewMode;
  thumbnailOrientation: ThumbnailOrientation;
  selectedIds: string[];
  activeId?: string;
  inspectorOpen: boolean;
  leftPanelOpen: boolean;
  settingsOpen: boolean;
  settingsSection: SettingsSection;
  openSettings: (section?: SettingsSection) => void;
  locale: Locale;
  navigatorVisible: boolean;
  navigatorPosition: NavigatorPosition;
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
  pickLabels: string[];
  sort: AssetSort;
  direction: SortDirection;
  shortcuts: ShortcutBindings;
  setShortcut: (action: ShortcutAction, binding: ShortcutBinding) => void;
  resetShortcuts: () => void;
  setView: (view: ViewMode) => void;
  setThumbnailOrientation: (orientation: ThumbnailOrientation) => void;
  select: (id: string, additive?: boolean) => void;
  clearSelection: () => void;
  toggleInspector: () => void;
  toggleLeftPanel: () => void;
  toggleSettings: () => void;
  setSettingsSection: (section: SettingsSection) => void;
  setLocale: (locale: Locale) => void;
  setNavigatorVisible: (visible: boolean) => void;
  setNavigatorPosition: (position: NavigatorPosition) => void;
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
  togglePickLabel: (label: string) => void;
  setSort: (sort: AssetSort) => void;
  toggleDirection: () => void;
}

export type SettingsSection = "general" | "display" | "media" | "externalApps" | "shortcuts";

export const useWorkspaceStore = create<WorkspaceState>((set) => ({
  view: "grid",
  thumbnailOrientation: "landscape",
  selectedIds: [],
  inspectorOpen: true,
  leftPanelOpen: true,
  settingsOpen: false,
  settingsSection: "general",
  openSettings: (settingsSection = "general") => set({ settingsOpen: true, settingsSection }),
  locale: "zh-CN",
  navigatorVisible: true,
  navigatorPosition: "bottom-right",
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
  pickLabels: [],
  sort: "name",
  direction: "ascending",
  shortcuts: loadShortcuts(),
  setShortcut: (action, binding) =>
    set((state) => {
      const shortcuts = { ...state.shortcuts, [action]: binding };
      saveShortcuts(shortcuts);
      return { shortcuts };
    }),
  resetShortcuts: () => {
    const shortcuts = { ...DEFAULT_SHORTCUTS };
    saveShortcuts(shortcuts);
    set({ shortcuts });
  },
  setView: (view) => set({ view }),
  setThumbnailOrientation: (thumbnailOrientation) => set({ thumbnailOrientation }),
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
  toggleSettings: () => set((state) => ({ settingsOpen: !state.settingsOpen, settingsSection: "general" })),
  setSettingsSection: (settingsSection) => set({ settingsSection }),
  setLocale: (locale) => set({ locale }),
  setNavigatorVisible: (navigatorVisible) => set({ navigatorVisible }),
  setNavigatorPosition: (navigatorPosition) => set({ navigatorPosition }),
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
  togglePickLabel: (label) => set((state) => ({
    pickLabels: state.pickLabels.includes(label)
      ? state.pickLabels.filter((pickLabel) => pickLabel !== label)
      : [...state.pickLabels, label],
  })),
  setSort: (sort) => set({ sort }),
  toggleDirection: () =>
    set((state) => ({
      direction: state.direction === "ascending" ? "descending" : "ascending",
    })),
}));
