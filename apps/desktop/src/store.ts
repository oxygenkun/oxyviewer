import { create } from "zustand";
import type { Locale } from "./lib/i18n";
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
  search: string;
  kind?: AssetKind;
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
  setSearch: (search: string) => void;
  setKind: (kind?: AssetKind) => void;
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
  search: "",
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
  setSearch: (search) => set({ search }),
  setKind: (kind) => set({ kind }),
  setSort: (sort) => set({ sort }),
  toggleDirection: () =>
    set((state) => ({
      direction: state.direction === "ascending" ? "descending" : "ascending",
    })),
}));
