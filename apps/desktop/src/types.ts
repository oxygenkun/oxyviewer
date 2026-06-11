export type AssetKind = "raw" | "jpeg" | "heif" | "png" | "tiff" | "webp";
export type AssetSort = "name" | "modified" | "size" | "kind";
export type SortDirection = "ascending" | "descending";
export type ViewMode = "grid" | "list" | "loupe";
export type GridPreference = "landscape" | "portrait";
export type NavigatorPosition = "top-left" | "top-right" | "bottom-left" | "bottom-right";

export interface FolderSession {
  id: string;
  rootPath: string;
  displayName: string;
  openedAtMs: number;
}

export interface DirectorySummary {
  path: string;
  name: string;
  hasChildren: boolean;
}

export interface AssetSummary {
  id: string;
  path: string;
  name: string;
  extension: string;
  kind: AssetKind;
  sizeBytes: number;
  modifiedAtMs: number;
  hasSidecar: boolean;
}

export interface EditableMetadata {
  rating?: number;
  colorLabel?: string;
  title?: string;
  description?: string;
  creator?: string;
  copyright?: string;
  keywords: string[];
}

export interface AssetDetails {
  asset: AssetSummary;
  width?: number;
  height?: number;
  metadata: EditableMetadata;
  sidecarPath?: string;
}

export type PreviewMode = "thumbnail" | "loupePreview" | "fullDetail";
export type PreviewKind = "embedded" | "developed" | "decoded" | "system" | "original";

export interface PreviewResult {
  path: string;
  url: string;
  width: number;
  height: number;
  kind: PreviewKind;
}

export interface AssetQuery {
  search?: string;
  kind?: AssetKind;
  sort: AssetSort;
  direction: SortDirection;
  pageSize: number;
}

export interface Page<T> {
  items: T[];
  nextCursor?: number;
  total: number;
}
