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

export interface AssetTagAssignmentsByPath {
  path: string;
  assignments: AssetTagAssignment[];
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
  workerWaitMicros: number;
  collectionMicros: number;
  staleQueues: string[];
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
  sourceReadBytes?: number;
  sourceReadCalls?: number;
  probeMs?: number;
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

export interface PixelDimensions {
  width: number;
  height: number;
}

export type ImageOrigin = "primaryImage" | "embeddedPreview" | "rawSensor";
export type ImageOperation =
  | { operation: "decode" | "develop"; backend: string }
  | { operation: "resize"; from: PixelDimensions; to: PixelDimensions }
  | { operation: "orient"; exif: number }
  | { operation: "encode"; format: string }
  | { operation: "colorConvert"; target: string }
  | { operation: "sharpen" | "metadataEdit" }
  | { operation: "crop"; region: PreviewGeometry["contentRect"] }
  | { operation: "pad"; canvas: PixelDimensions }
  | { operation: "assemble"; mode: string };

/** Actual content identity and completed processing, independent of render level. */
export interface ArtifactFacts {
  exifOrientation: number;
  source: {
    exifOrientation: number;
    revisionId: string;
    candidateId: string;
    origin: ImageOrigin;
    encodedDimensions: PixelDimensions | null;
    displayDimensions: PixelDimensions;
  };
  encodedDimensions: PixelDimensions;
  displayDimensions: PixelDimensions;
  detail: {
    referenceDimensions: PixelDimensions;
    region: PreviewGeometry["contentRect"];
    sampledDimensions: PixelDimensions;
    sampling: "native" | "reduced";
  };
  processing: ImageOperation[];
  byteIntegrity: "sourceFile" | "sourcePayload" | "metadataAdjusted" | "reencoded" | "unverified";
}

export interface PreviewResult {
  imageFacts?: ArtifactFacts;
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

/** Build metadata rendered by the About settings panel. */
export interface AppInfo {
  name: string;
  version: string;
  repositoryUrl: string;
  author: string | null;
  license: string | null;
}

/** Result of one user-requested release check. Advisory only; nothing installs. */
export interface UpdateStatus {
  currentVersion: string;
  latestVersion: string;
  updateAvailable: boolean;
  releaseUrl: string;
  releaseName?: string;
  publishedAt?: string;
}

/** Fixed external destinations the About panel may open. */
export type AboutLink = "repository" | "releases" | "license";

export interface RawDecoderStatus {
  installAvailable: boolean;
  availability: "available" | "missing" | "unavailable" | "unsupportedPlatform";
  codecs: { name: string; decoderId: string; version: string; extensions: string }[];
  detail: string | null;
  attempt: { state: "ready" | "unsupportedFile" | "failed"; detail: string | null } | null;
}

export interface AssetQuery {
  tagIds?: number[];
  tagMatch?: "all" | "any";
  search?: string;
  kind?: AssetKind;
  minimumRating?: number;
  colorLabels?: string[];
  pickLabels?: string[];
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

export interface WindowDragPosition {
  x: number;
  y: number;
}

/** Normalized native drag-and-drop payload from the desktop window. */
export type WindowDragEvent =
  | { type: "enter"; paths: string[]; position: WindowDragPosition }
  | { type: "over"; position: WindowDragPosition }
  | { type: "drop"; paths: string[]; position: WindowDragPosition }
  | { type: "leave" };

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
  expectedAssets?: number;
  name: string;
  folder: string;
  selectName?: string;
  enterLoupe?: boolean;
  scrollToEnd?: boolean;
  resourceStress?: "faces-workbench" | "faces-browsing" | "loupe-zoom" | "grid" | "list" | "loupe" | "grid-scroll" | "filmstrip-scroll" | "navigation-cache" | "folder-thumbnails";
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
export interface ExternalApplication {
  id: string;
  name: string;
  executablePath: string;
}

export interface ExternalAppSettings {
  apps: ExternalApplication[];
  defaultAppId: string | null;
}

export type ExternalOpenResult = "launched" | "cancelled";

/* ---------------------------------------------------------------------------
 * Face analysis and people.
 *
 * Mirrors `oxy-domain::faces`. Machine output (observations, candidates,
 * clusters) is rebuildable; persons and decisions are user data that a model
 * change must never destroy.
 * ------------------------------------------------------------------------- */

export interface NormalizedRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface NormalizedPoint {
  x: number;
  y: number;
}

/** One detected face. Rebuildable analyzer output. */
export interface FaceObservation {
  observationId: string;
  assetId: string;
  assetPath: string;
  sourceRevision: string;
  localIndex: number;
  bbox: NormalizedRect;
  landmarks: NormalizedPoint[];
  detectionScore: number;
  detectorFingerprint: string;
}

export type FaceReviewState = "unknown" | "pending" | "confirmed" | "rejected" | "notFace";

/** A matcher proposal awaiting a user answer. */
export interface FaceCandidate {
  observationId: string;
  personId: string;
  personName: string;
  similarity: number;
  matcherFingerprint: string;
}

/** A stable person. Renaming never invalidates an assignment. */
export interface Person {
  personId: string;
  displayName: string;
  linkedTagId?: number;
  createdAtMs: number;
  updatedAtMs: number;
  faceCount: number;
  coverObservationId?: string;
}

/** A suggestion that several unknown faces show one person. Cache, not identity. */
export interface FaceCluster {
  clusterId: string;
  observationIds: string[];
  representativeObservationId: string;
  memberCount: number;
  cohesion: number;
  /** Members that sit far from the rest; leave them unselected on confirm. */
  outlierObservationIds: string[];
  suggestedPersonId?: string;
  suggestedName?: string;
  memberQuality?: FaceQuality[];
}

export interface FaceQuality {
  observationId: string;
  facePixels: number;
  clarity: number;
  detectionScore: number;
}

export interface FaceReviewItem {
  observationId: string;
  assetId: string;
  assetPath: string;
  bbox: NormalizedRect;
  detectionScore: number;
  facePixels?: number;
  clarity?: number;
  manualBlurry?: boolean;
  state: FaceReviewState;
  candidate?: FaceCandidate;
  clusterId?: string;
  confirmedPersonId?: string;
  confirmedPersonName?: string;
}

export interface FaceReviewPage {
  items: FaceReviewItem[];
  total: number;
  nextCursor: number | null;
}

/** What the user asserted about one face region. Mirrors the Rust enum tag. */
export type FaceDecision =
  | { decision: "confirmPerson"; personId: string }
  | { decision: "rejectPerson"; personId: string }
  | { decision: "notFace" };

export type FaceMatchSensitivity = "strict" | "balanced" | "loose" | "custom";

export interface FaceAnalyzerSettings {
  detectionConfidence: number;
  nmsThreshold: number;
  maxFacesPerAsset: number;
  minFacePixels: number;
  /** Analyze overlapping tiles as well, to keep small faces detectable. */
  detectSmallFaces: boolean;
  matchSensitivity: FaceMatchSensitivity;
  matchThreshold: number;
  autoAcceptThreshold?: number;
  clusterThreshold: number;
}

export type FaceAnalysisStage =
  | "idle"
  | "detecting"
  | "embedding"
  | "clustering"
  | "matching"
  | "complete"
  | "failed";

export interface FaceAnalysisProgress {
  jobId: string;
  stage: FaceAnalysisStage;
  processedAssets: number;
  totalAssets: number;
  facesDetected: number;
  pendingReviews: number;
  /** Files that failed to decode or analyze; they produced no faces. */
  failedAssets: number;
  message?: string;
}

export interface FaceAnalysisRequest {
  /** Explicit selection; wins over the directory scope. */
  paths?: string[];
  /** Browsed directory, as the folder session holds it. */
  rootPath?: string;
  directory?: string;
  force?: boolean;
}

export interface FaceLibraryStats {
  analyzedAssets: number;
  facesDetected: number;
  persons: number;
  pendingReviews: number;
  unknownFaces: number;
  clusters: number;
}

export interface FaceCapability {
  available: boolean;
  unavailableReason?: string;
  running: boolean;
  settings: FaceAnalyzerSettings;
  stats: FaceLibraryStats;
  progress?: FaceAnalysisProgress;
  models: FaceModelStatus[];
  /** File that holds persons and confirmations; user data, not cache. */
  peopleStorePath: string;
}

export interface FaceModelStatus {
  id: "scrfd-10g-kps" | "adaface-ir101" | string;
  displayName: string;
  installed: boolean;
  sizeBytes: number;
  downloadSizeBytes: number;
  licenseSummary: string;
}

export type FaceModelDownloadStage = "downloading" | "verifying" | "installing" | "complete" | "failed";

export interface FaceModelDownloadProgress {
  modelId: string;
  stage: FaceModelDownloadStage;
  downloadedBytes: number;
  totalBytes: number;
  message?: string;
}

/**
 * How the matcher behaves on this library, measured from the user's answers.
 *
 * The samples are biased: only faces the current threshold promoted to a
 * candidate could be accepted or rejected, so this refines the current setting
 * rather than replacing it outright.
 */
export interface FaceCalibration {
  acceptedScores: number[];
  rejectedScores: number[];
  currentThreshold: number;
  recommendedThreshold?: number;
  separable: boolean;
}

/** The newest reversible person operation, if any. */
export interface UndoableOperation {
  kind: "mergePersons" | "removeFaces" | "assignFaces" | string;
  personId: string;
  faceCount: number;
  otherPersonName?: string;
}

/** One face crop delivered as a leaseable media resource. */
export interface FaceCropResource {
  observationId: string;
  descriptor: {
    resourceId: string;
    url: string;
    mediaType: string;
  };
}

/**
 * The asset a face was detected on.
 *
 * A crop is keyed by observation, but "show me this photo" needs the asset, so
 * the workbench resolves one before asking the main window to reveal it.
 */
export interface FaceAssetReveal {
  observationId: string;
  assetId: string;
  assetPath: string;
}

/** One directory the workbench may scope a run to. */
export interface FaceWorkbenchScope {
  rootPath: string;
  directory: string;
}

/**
 * What the main window publishes to the face workbench window.
 *
 * The workbench has its own store, so the browse scope, the loaded selection,
 * and the locale cross the window boundary as published state.
 */
export interface FaceWorkbenchContext {
  locale: "zh-CN" | "en";
  visiblePaths: string[];
  browseScope?: FaceWorkbenchScope;
}

export interface OverlayRegion {
  id: string;
  rect: { x: number; y: number; width: number; height: number };
  label?: string;
  state?: string;
  actionRef?: string;
}
export interface OverlayDescriptor {
  id: string;
  coordinateSpace: "displayNormalized";
  items: OverlayRegion[];
}
export interface CollectionViewDescriptor {
  id: string;
  source: string;
  selection: "single" | "multi";
  fields: Array<{ key: string; label: string; kind: "text" | "percent" }>;
  actions: string[];
}
export interface SettingsDescriptor {
  id: string;
  fields: Array<{ key: string; label: string; recompute: "detection" | "matching" | "clustering" } & (
    { type: "number"; min: number; max: number; step: number } | { type: "boolean" } | { type: "enum"; values: string[] }
  )>;
}
export interface PortableFaceFact {
  personTagPath?: string;
  id: string;
  revision: string;
  region: NormalizedRect;
  decision?: FaceDecision | null;
  personName?: string | null;
}
export interface FaceSyncStatus {
  path: string; pending: boolean; conflict: boolean; lastError?: string | null;
  localFacts?: { facts: PortableFaceFact[] } | null;
  remoteFacts?: { facts: PortableFaceFact[] } | null;
}
