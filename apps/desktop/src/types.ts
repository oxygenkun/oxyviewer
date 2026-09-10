export type AssetKind = "raw" | "jpeg" | "heif" | "png" | "tiff" | "webp";
export type AssetSort = "name" | "modified" | "size" | "kind";
export type SortDirection = "ascending" | "descending";
export type ViewMode = "grid" | "list" | "loupe";
export type ThumbnailOrientation = "landscape" | "portrait";
export type NavigatorPosition = "top-left" | "top-right" | "bottom-left" | "bottom-right";
export type PickLabel = "rejected" | "pending" | "accepted";
export type FileDeletionMode = "trash" | "permanent";

export interface FolderSession {
  id: string;
  rootPath: string;
  displayName: string;
  openedAtMs: number;
  deletionMode: FileDeletionMode;
}

export interface DirectorySummary {
  path: string;
  name: string;
  hasChildren: boolean;
}

export interface DirectoryTreeNode {
  entry: DirectorySummary;
  expanded: boolean;
  children: DirectoryTreeNode[] | null;
}

export interface DirectoryTreeSnapshot {
  sessionId: string;
  revision: number;
  root: DirectoryTreeNode;
}

export interface DirectorySearchMatch {
  directory: DirectorySummary;
  ancestors: DirectorySummary[];
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
  rating?: number;
  colorLabel?: string;
  pickLabel?: PickLabel;
}

export interface EditableMetadata {
  rating?: number;
  colorLabel?: string;
  pickLabel?: PickLabel;
  title?: string;
  description?: string;
  creator?: string;
  copyright?: string;
  keywords: string[];
  hierarchicalKeywords: string[];
}

export interface MetadataPatch {
  rating?: number | null;
  colorLabel?: string | null;
  pickLabel?: PickLabel | null;
  keywords?: string[];
  hierarchicalKeywords?: string[];
}

export interface CustomTag {
  id: number;
  parentId?: number;
  name: string;
  path: string;
  sortOrder: number;
}

export interface AssetTagAssignment {
  tag: CustomTag;
  assignedCount: number;
  assetCount: number;
}

export interface TagDeleteImpact {
  tagCount: number;
  assetCount: number;
}

export interface TagSyncStatus {
  pendingCount: number;
  failedCount: number;
  lastError?: string;
}

export interface MetadataCapability {
  provider: "sidecar" | "native" | "exiftool";
  readable: boolean;
  writable: boolean;
  detail?: string;
}

export interface ExiftoolStatus {
  available: boolean;
  source?: "user" | "managed" | "configured" | "path";
  version?: string;
  executablePath?: string;
  detail?: string;
}

export interface FocusRegion {
  centerX: number;
  centerY: number;
  width?: number;
  height?: number;
}

export interface FocusInfo {
  coordinateWidth: number;
  coordinateHeight: number;
  regions: FocusRegion[];
}

export interface CaptureMetadata {
  aperture?: string;
  exposureTime?: string;
  focalLength?: string;
  iso?: string;
  exposureCompensation?: string;
  capturedAt?: string;
  cameraMake?: string;
  cameraModel?: string;
  lensMake?: string;
  lensModel?: string;
  chromaSubsampling?: string;
  colorTemperature?: string;
  tint?: string;
  dynamicRangeOptimizer?: string;
}

export interface AssetDetails {
  asset: AssetSummary;
  width?: number;
  height?: number;
  metadata: EditableMetadata;
  embeddedKeywords: string[];
  embeddedHierarchicalKeywords: string[];
  metadataCapability: MetadataCapability;
  sidecarPath?: string;
  captureMetadata: CaptureMetadata;
  focusInfo?: FocusInfo;
}

export type ResourceLoadStatus = "loading" | "ready" | "error";
export type MetadataRequestPriority = "background" | "filter" | "visible" | "selected";

export interface MetadataProjection {
  path: string;
  sourceRevision: string;
  stateRevision: number;
  validAt: number;
  status: ResourceLoadStatus;
  rating?: number;
  colorLabel?: string;
  pickLabel?: PickLabel;
  error?: string;
}

export interface AssetDetailsResult {
  details: AssetDetails;
  metadataProjection: MetadataProjection;
}

export type RenderLevel = "thumbnail" | "preview" | "full";
export type PreviewPriority = "preload" | "nearby" | "visible" | "loupe";

export interface DebugQueueItem {
  key: string;
  path?: string;
  rootPath?: string;
  resource?: string;
  stage: string;
  priority: string;
  rank?: number;
  consumers: number;
  pendingCount?: number;
  assetCount?: number;
  directoryCount?: number;
}

export interface DebugQueueState {
  name: string;
  concurrency: number;
  pending: DebugQueueItem[];
  active: DebugQueueItem[];
}

export interface DebugQueueSnapshot {
  capturedAtUnixMs: number;
  queues: DebugQueueState[];
}
export type SchedulePlacement = "front" | "back";

export interface PreviewScheduleIntent {
  path: string;
  level: RenderLevel;
  priority: PreviewPriority;
  rank: number;
}

export type PreviewOmittedPolicy =
  | { action: "release" }
  | {
      action: "demote";
      priority: PreviewPriority;
      placement: SchedulePlacement;
    };
export type PreviewKind = "embedded" | "developed" | "decoded" | "system" | "original";

export interface PreviewDiagnostics {
  backend?: string;
  queueWaitMs?: number;
  sourceWaitMs?: number;
  decodeMs?: number;
  encodeMs?: number;
  cacheSyncMs?: number;
  cacheCommitMs?: number;
  totalMs?: number;
  fallbackReason?: string;
}

export type MediaSatisfaction = "satisfied" | "interim";
export type MediaPersistence = "notApplicable" | "pending" | "persisted" | "skipped";

export interface MediaResourceDescriptor {
  resourceId: string;
  url: string;
  mediaType: string;
}

export interface PreviewGeometry {
  /** Full display-oriented canvas; does not describe the preview's resolution. */
  displaySize: { width: number; height: number };
  /** Display-oriented raster pixel edges, covering the complete logical canvas. */
  contentRect: { x: number; y: number; width: number; height: number };
}

export interface PreviewResult {
  geometry?: PreviewGeometry;
  path: string;
  url: string;
  width: number;
  height: number;
  kind: PreviewKind;
  renderLevel: RenderLevel;
  resource?: MediaResourceDescriptor;
  satisfaction?: MediaSatisfaction;
  persistence?: MediaPersistence;
  diagnostics?: PreviewDiagnostics;
}

export interface ImageProjection {
  path: string;
  sourceRevision: string;
  stateRevision: number;
  validAt: number;
  status: ResourceLoadStatus;
  level: RenderLevel;
  result?: Omit<PreviewResult, "url">;
  error?: string;
}

export interface CacheSettings {
  location: string;
  defaultLocation: string;
  customParent?: string;
  isCustomLocation: boolean;
  maxSizeBytes: number;
  usedSizeBytes: number;
}

export interface AssetQuery {
  search?: string;
  kind?: AssetKind;
  minimumRating?: number;
  colorLabels?: string[];
  sort: AssetSort;
  direction: SortDirection;
  pageSize: number;
}

export interface Page<T> {
  items: T[];
  nextCursor?: number;
  total: number;
  progress?: DirectoryBrowseProgress;
  snapshotRevision?: number;
}

export interface DirectoryBrowseProgress {
  sessionId: string;
  directory: string;
  stage: string;
  source: string;
  discoveredCount: number;
  elapsedMs: number;
  cacheMs: number;
  resolveMs: number;
  enumerationMs: number;
  attributesMs: number;
  snapshotSerializeMs: number;
  snapshotPersistMs: number;
  sortMs: number;
  error?: string;
}

export interface LibraryIndexUpdate {
  rootPath: string;
  assetCount: number;
  directoryCount: number;
}

export type HeifBackendKind =
  | "cachedArtifact"
  | "windowsWic"
  | "appleImageIo"
  | "linuxVaapi"
  | "ffmpegSoftware"
  | "libheifSoftware";
export type AccelerationKind = "hardware" | "software" | "unknown";
export type HeifDecodeStatus =
  | "decoding"
  | "complete"
  | "failed"
  | "cancelled";

export interface HeifCapabilities {
  backend: HeifBackendKind;
  acceleration: AccelerationKind;
  available: boolean;
  detail?: string;
}

export interface HeifDecodeSession {
  id: string;
  generation: number;
  width: number;
  height: number;
  tileSize: number;
  expectedTiles: number;
  backend: HeifBackendKind;
  acceleration: AccelerationKind;
  status: HeifDecodeStatus;
}

export type HeifFullPresentation =
  | { delivery: "artifact"; projection: ImageProjection }
  | { delivery: "tiles"; session: HeifDecodeSession };

export interface HeifTileReady {
  sessionId: string;
  generation: number;
  x: number;
  y: number;
  width: number;
  height: number;
  payload: "rgba" | "jpeg";
  url: string;
}

export interface HeifDiagnostics {
  backend: HeifBackendKind;
  acceleration: AccelerationKind;
  codec?: string;
  queueWaitMs: number;
  decodeMs: number;
  tilePublishMs: number;
  totalMs: number;
  fallbackReason?: string;
}

export interface HeifStatusEvent {
  sessionId: string;
  generation: number;
  status: HeifDecodeStatus;
  diagnostics?: HeifDiagnostics;
  message?: string;
}

/**
 * One automated performance scenario injected by the E2E runner through the
 * `OXY_PERF_SCENARIO` environment variable. Mirrors `PerfScenario` in
 * `oxy-domain`; see `docs/PERF_E2E.md`.
 */
export interface PerfScenario {
  name: string;
  folder: string;
  selectName?: string;
  enterLoupe?: boolean;
  scrollToEnd?: boolean;
  resourceStress?: "grid" | "list" | "loupe" | "grid-scroll" | "filmstrip-scroll" | "navigation-cache";
  awaitMarks: string[];
  timeoutMs?: number;
  reportPath: string;
}

export interface PerfMark {
  name: string;
  /** Milliseconds relative to page load, from performance.now(). */
  t: number;
  detail?: Record<string, unknown>;
}

/** Native registry counters; filesystem payload sizes are separate from memory. */
export interface ResourceRegistryStats {
  maxEntries: number;
  maxEncodedBytes: number;
  maxMaterializedResponses: number;
  maxMaterializedBytes: number;
  entries: number;
  encodedBytes: number;
  peakEntries: number;
  peakEncodedBytes: number;
  uiLeased: number;
  readLeased: number;
  releasedOrExpired: number;
  stagedFiles: number;
  stagedBytes: number;
  materializedResponses: number;
  materializedBytes: number;
}
