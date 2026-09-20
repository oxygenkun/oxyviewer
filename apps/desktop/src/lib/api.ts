import { ensureTagPath, normalizeCustomTag } from "@/lib/assets/tagTree";
import { retainMediaResource, releaseUnretainedMediaResource } from "@/lib/cache/mediaResourceLease";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AboutLink,
  AppInfo,
  ExternalAppSettings,
  ExternalOpenResult,
  RawDecoderStatus,
  UpdateStatus,
  AssetDetails,
  AssetDetailsResult,
  AssetKind,
  AssetQuery,
  AssetSummary,
  CacheSettings,
  DirectorySearchMatch,
  DirectoryBrowseProgress,
  DirectorySummary,
  DirectoryTreeNode,
  DirectoryTreeSnapshot,
  FolderSession,
  HeifCapabilities,
  HeifFullPresentation,
  ImageProjection,
  LibraryIndexUpdate,
  ExiftoolStatus,
  FileDeletionMode,
  MetadataPatch,
  MetadataProjection,
  MetadataRequestPriority,
  Page,
  PerfScenario,
  PreviewPriority,
  PreviewOmittedPolicy,
  PreviewResult,
  PreviewScheduleIntent,
  RenderLevel,
  SchedulePlacement,
  AssetTagAssignment,
  AssetTagAssignmentsByPath,
  CustomTag,
  DebugQueueSnapshot,
  FaceAnalysisProgress,
  FaceAnalysisRequest,
  FaceAnalyzerSettings,
  FaceAssetReveal,
  FaceCalibration,
  FaceCapability,
  FaceCluster,
  FaceCropResource,
  FaceDecision,
  FaceLibraryStats,
  FaceModelDownloadProgress,
  FaceObservation,
  FaceReviewItem,
  FaceReviewPage,
  FaceWorkbenchContext,
  Person,
  UndoableOperation,
  TagDeleteImpact,
  TagSyncStatus,
  WindowDragEvent,
} from "@/types";
import { getFolderThumbnail, getFolderThumbnailGeneration, preloadFolderThumbnail } from "@/lib/cache/folderThumbnailCache";
import { browserPreloadQueue, orderedPriorityWeight } from "@/lib/preview/previewQueue";
import { acceptImageProjection } from "@/lib/projection/imageProjection";
import { mediaProtocolUrl } from "@/lib/media/mediaProtocolUrl";
import { acceptMetadataProjection } from "@/lib/projection/metadataProjection";
import { perfMark } from "@/lib/diagnostics/perfProbe";
import { recordBrowseTiming } from "@/lib/diagnostics/browseDiagnostics";
import { beginPreviewDebug } from "@/lib/diagnostics/previewDebug";
import { sharedThumbnailRequests } from "@/lib/preview/sharedThumbnailRequests";

function cancelGeneratedPreviewRequest(
  asset: Pick<AssetSummary, "path">,
  level: RenderLevel,
  requestId: string,
): void {
  void invoke("cancel_preview_request", {
    path: asset.path,
    level,
    requestId,
  }).catch(() => undefined);
}

const demoNames: Array<[string, AssetKind, number]> = [
  ["DSC_4281.NEF", "raw", 42_840_312],
  ["coastline-dawn.CR3", "raw", 51_220_840],
  ["quiet-corner.jpg", "jpeg", 8_340_120],
  ["IMG_7844.HEIC", "heif", 5_892_401],
  ["field-notes.ARW", "raw", 46_270_032],
  ["studio-portrait.jpg", "jpeg", 12_420_119],
  ["night-platform.DNG", "raw", 62_992_811],
  ["water-study.jpg", "jpeg", 9_871_421],
  ["IMG_7912.HEIC", "heif", 6_228_830],
  ["IMG_7920.HIF", "heif", 6_520_440],
  ["market-light.RAF", "raw", 53_009_210],
  ["silver-grain.jpg", "jpeg", 11_084_120],
  ["still-life.ORF", "raw", 24_340_091],
  ["blue-hour.RW2", "raw", 37_771_400],
  ["archive-scan.tiff", "tiff", 88_770_100],
  ["contact-sheet.webp", "webp", 3_120_889],
  ["proof-select.jpg", "jpeg", 10_840_204],
  ["mountain-air.NEF", "raw", 41_128_991],
  ["window-light.jpg", "jpeg", 7_029_402],
];

const demoRoot = "/demo/Field Notes";
const demoDirectories: DirectorySummary[] = [
  { path: `${demoRoot}/Portraits`, name: "Portraits", hasChildren: true },
  { path: `${demoRoot}/Trips`, name: "Trips", hasChildren: true },
  { path: `${demoRoot}/Trips/Coast`, name: "Coast", hasChildren: true },
];
let demoTreeRevision = 0;
const demoDirectoryTrees = new Map<string, DirectoryTreeSnapshot>();
let demoTagId = 3;
let demoTags: CustomTag[] = [
  { id: 1, name: "人物", path: "人物", sortOrder: 0 },
  { id: 2, parentId: 1, name: "家人", path: "人物|家人", sortOrder: 0 },
];
const demoAssetTags = new Map<string, Set<number>>();
const demoFaceOwnedTags = new Map<string, Set<number>>();

function demoTreeChildren(path: string, previous: DirectoryTreeNode[] = []): DirectoryTreeNode[] {
  const previousByPath = new Map(previous.map((node) => [node.entry.path, node]));
  return demoDirectories
    .filter((entry) => entry.path.slice(0, entry.path.lastIndexOf("/")) === path)
    .map((entry) => previousByPath.get(entry.path) ?? {
      entry,
      expanded: false,
      children: null,
    });
}

function findDemoTreeNode(node: DirectoryTreeNode, path: string): DirectoryTreeNode | undefined {
  if (node.entry.path === path) return node;
  return node.children?.map((child) => findDemoTreeNode(child, path)).find(Boolean);
}

function cloneDemoTree(snapshot: DirectoryTreeSnapshot): DirectoryTreeSnapshot {
  const cloneNode = (node: DirectoryTreeNode): DirectoryTreeNode => ({
    entry: { ...node.entry },
    expanded: node.expanded,
    children: node.children?.map(cloneNode) ?? null,
  });
  return { ...snapshot, root: cloneNode(snapshot.root) };
}

function createDemoTree(session: FolderSession): DirectoryTreeSnapshot {
  const snapshot: DirectoryTreeSnapshot = {
    sessionId: session.id,
    revision: ++demoTreeRevision,
    root: {
      entry: { path: session.rootPath, name: session.displayName, hasChildren: true },
      expanded: false,
      children: null,
    },
  };
  demoDirectoryTrees.set(session.id, snapshot);
  return snapshot;
}

const demoAssets: AssetSummary[] = demoNames.map(([name, kind, sizeBytes], index) => ({
  id: `demo-${index}`,
  path: `${index < 10 ? demoRoot : index < 15 ? `${demoRoot}/Portraits` : `${demoRoot}/Trips/Coast`}/${name}`,
  name,
  extension: name.split(".").at(-1)?.toUpperCase() ?? "",
  kind,
  sizeBytes,
  modifiedAtMs: Date.now() - index * 3_600_000,
  hasSidecar: kind === "raw" && index % 3 !== 1,
  rating: index % 6 || undefined,
  colorLabel: ["Red", "Yellow", "Green", "Blue", "Purple"][index % 7],
  pickLabel: index % 5 === 0 ? "accepted" : index % 7 === 0 ? "rejected" : undefined,
}));

export const isTauri = () => "__TAURI_INTERNALS__" in window;

export async function chooseFolder(): Promise<string | null> {
  if (!isTauri()) return "/demo/Field Notes";
  const selection = await open({ directory: true, multiple: false });
  return typeof selection === "string" ? selection : null;
}

export async function openFolder(path: string): Promise<FolderSession> {
  if (!isTauri()) {
    // The browser demo can open more than one folder, so the session must be
    // keyed by path rather than a single shared id.
    const displayName = path.replace(/[\\/]+$/, "").split(/[\\/]/).at(-1) || "Field Notes";
    const session: FolderSession = {
      id: `demo-${path}`,
      rootPath: path,
      displayName,
      openedAtMs: Date.now(),
      deletionMode: "trash",
    };
    createDemoTree(session);
    return session;
  }
  perfMark("folder:open-requested", { path });
  const session = await invoke<FolderSession>("open_folder", { path });
  perfMark("folder:open-returned", { sessionId: session.id });
  return session;
}

export async function listAssets(
  sessionId: string,
  directory: string,
  query: AssetQuery,
  cursor?: number,
  snapshotRevision?: number,
): Promise<Page<AssetSummary>> {
  if (!isTauri()) {
    const needle = query.search?.toLowerCase();
    const tagGroups = (query.tagIds ?? []).map((id) => {
      const root = demoTags.find((tag) => tag.id === id);
      return new Set(root ? demoTags.filter((tag) => tag.id === id || tag.path.startsWith(`${root.path}|`)).map((tag) => tag.id) : []);
    });
    const filtered = [...demoAssets]
      .filter((asset) => !tagGroups.length || (query.tagMatch === "any"
        ? tagGroups.some((group) => [...(demoAssetTags.get(asset.path) ?? [])].some((id) => group.has(id)))
        : tagGroups.every((group) => [...(demoAssetTags.get(asset.path) ?? [])].some((id) => group.has(id)))))
      .filter((asset) => (!query.tagIds?.length && needle) || asset.path.slice(0, asset.path.lastIndexOf("/")) === directory)
      .filter((asset) => !query.kind || asset.kind === query.kind)
      .filter((asset) => !query.minimumRating || (asset.rating ?? 0) >= query.minimumRating)
      .filter((asset) => !query.colorLabels?.length ||
        query.colorLabels.some((label) => asset.colorLabel?.toLowerCase() === label.toLowerCase()))
      .filter((asset) => !query.pickLabels?.length ||
        query.pickLabels.some((label) => asset.pickLabel?.toLowerCase() === label.toLowerCase()))
      .filter((asset) => !needle || asset.name.toLowerCase().includes(needle))
      .sort((left, right) => {
        const multiplier = query.direction === "ascending" ? 1 : -1;
        if (query.sort === "size") return (left.sizeBytes - right.sizeBytes) * multiplier;
        if (query.sort === "modified")
          return (left.modifiedAtMs - right.modifiedAtMs) * multiplier;
        return left.name.localeCompare(right.name) * multiplier;
      });
    const start = cursor ?? 0;
    const end = Math.min(start + query.pageSize, filtered.length);
    return {
      items: filtered.slice(start, end),
      nextCursor: end < filtered.length ? end : undefined,
      total: filtered.length,
    };
  }
  const started = performance.now();
  const page = await invoke<Page<AssetSummary>>("list_assets", {
    sessionId,
    directory,
    query,
    cursor,
    snapshotRevision,
  });
  if (!cursor) {
    recordBrowseTiming("first-page-returned", { directory, ipcMs: performance.now() - started, ...page.progress });
    perfMark("assets:first-page-returned", { directory, total: page.total, ipcMs: performance.now() - started, ...page.progress });
  }
  return page;
}

export async function onLibraryIndexUpdated(
  callback: (update: LibraryIndexUpdate) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<LibraryIndexUpdate>("library-index-updated", (event) => callback(event.payload));
}

/**
 * Native drag-and-drop from the OS. Tauri intercepts the platform drop
 * (`dragDropEnabled` defaults to on), so HTML5 drag events never carry a real
 * path; this is the only channel that sees the dropped folders.
 */
export async function onWindowDragDrop(
  callback: (event: WindowDragEvent) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    const payload = event.payload;
    if (payload.type === "enter" || payload.type === "drop") {
      callback({
        type: payload.type,
        paths: payload.paths,
        position: { x: payload.position.x, y: payload.position.y },
      });
      return;
    }
    if (payload.type === "over") {
      callback({ type: "over", position: { x: payload.position.x, y: payload.position.y } });
      return;
    }
    callback({ type: "leave" });
  });
}

export async function onDirectoryBrowseProgress(callback: (progress: DirectoryBrowseProgress) => void): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<DirectoryBrowseProgress>("directory-browse-progress", (event) => callback(event.payload));
}

export async function onLibraryDirectoryIndexUpdated(
  callback: (update: LibraryIndexUpdate) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<LibraryIndexUpdate>("library-directory-index-updated", (event) => callback(event.payload));
}

export async function onDirectoryTreeUpdated(
  callback: (snapshot: DirectoryTreeSnapshot) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<DirectoryTreeSnapshot>("directory-tree-updated", (event) => callback(event.payload));
}

export async function getDirectoryTree(session: FolderSession): Promise<DirectoryTreeSnapshot> {
  if (!isTauri()) {
    const snapshot = demoDirectoryTrees.get(session.id) ?? createDemoTree(session);
    return cloneDemoTree(snapshot);
  }
  return invoke<DirectoryTreeSnapshot>("get_directory_tree", { sessionId: session.id });
}

export async function setDirectoryExpanded(
  sessionId: string,
  directory: string,
  expanded: boolean,
): Promise<DirectoryTreeSnapshot> {
  if (!isTauri()) {
    const snapshot = demoDirectoryTrees.get(sessionId);
    if (!snapshot) throw new Error(`Unknown folder session: ${sessionId}`);
    const node = findDemoTreeNode(snapshot.root, directory);
    if (!node) throw new Error(`Unknown directory tree node: ${directory}`);
    const knownLeaf = node.children?.length === 0;
    node.expanded = expanded && !knownLeaf;
    if (node.expanded && node.children === null) {
      node.children = demoTreeChildren(directory);
      node.entry.hasChildren = node.children.length > 0;
      if (node.children.length === 0) node.expanded = false;
    }
    snapshot.revision = ++demoTreeRevision;
    return cloneDemoTree(snapshot);
  }
  return invoke<DirectoryTreeSnapshot>("set_directory_expanded", {
    sessionId,
    directory,
    expanded,
  });
}

export async function collapseDirectoryTree(sessionId: string): Promise<DirectoryTreeSnapshot> {
  if (!isTauri()) {
    const snapshot = demoDirectoryTrees.get(sessionId);
    if (!snapshot) throw new Error(`Unknown folder session: ${sessionId}`);
    const collapseNode = (node: DirectoryTreeNode) => {
      node.expanded = false;
      node.children?.forEach(collapseNode);
    };
    collapseNode(snapshot.root);
    snapshot.revision = ++demoTreeRevision;
    return cloneDemoTree(snapshot);
  }
  return invoke<DirectoryTreeSnapshot>("collapse_directory_tree", { sessionId });
}

export async function setActiveDirectory(sessionId: string, directory: string): Promise<void> {
  if (!isTauri()) {
    if (!demoDirectoryTrees.has(sessionId)) {
      throw new Error(`Unknown folder session: ${sessionId}`);
    }
    void directory;
    return;
  }
  await invoke("set_active_directory", { sessionId, directory });
}

export async function searchDirectories(
  sessionId: string,
  search: string,
): Promise<DirectorySearchMatch[] | null> {
  if (!isTauri()) {
    const needle = search.trim().toLocaleLowerCase();
    if (!needle) return [];
    return demoDirectories
      .filter((entry) => entry.name.toLocaleLowerCase().includes(needle))
      .map((directory) => {
        const relativeParts = directory.path
          .slice(demoRoot.length)
          .split("/")
          .filter(Boolean);
        let ancestorPath = demoRoot;
        const ancestors = relativeParts.slice(0, -1).map((name) => {
          ancestorPath = `${ancestorPath}/${name}`;
          return { path: ancestorPath, name, hasChildren: true };
        });
        return { directory, ancestors };
      });
  }
  return invoke<DirectorySearchMatch[] | null>("search_directories", { sessionId, search });
}

export async function refreshDirectory(
  sessionId: string,
  directory: string,
): Promise<DirectoryTreeSnapshot> {
  if (!isTauri()) {
    const snapshot = demoDirectoryTrees.get(sessionId);
    if (!snapshot) throw new Error(`Unknown folder session: ${sessionId}`);
    const refreshNode = (node: DirectoryTreeNode) => {
      node.children = demoTreeChildren(node.entry.path, node.children ?? []);
      node.children.forEach(refreshNode);
      node.entry.hasChildren = node.children.length > 0;
      if (node.children.length === 0) node.expanded = false;
    };
    refreshNode(snapshot.root);
    snapshot.revision = ++demoTreeRevision;
    return cloneDemoTree(snapshot);
  }
  return invoke<DirectoryTreeSnapshot>("refresh_directory", { sessionId, directory });
}

export async function deletePaths(paths: string[], mode: FileDeletionMode): Promise<void> {
  if (!isTauri()) {
    for (let index = demoAssets.length - 1; index >= 0; index -= 1) {
      if (paths.some((path) => demoAssets[index].path === path || demoAssets[index].path.startsWith(`${path}/`))) {
        demoAssets.splice(index, 1);
      }
    }
    for (let index = demoDirectories.length - 1; index >= 0; index -= 1) {
      if (paths.some((path) => demoDirectories[index].path === path || demoDirectories[index].path.startsWith(`${path}/`))) {
        demoDirectories.splice(index, 1);
      }
    }
    return;
  }
  await invoke("execute_file_operation", {
    operation: { type: mode === "permanent" ? "deletePermanently" : "trash", paths },
  });
}

export async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const input = document.createElement("textarea");
  input.value = text;
  input.style.position = "fixed";
  input.style.opacity = "0";
  document.body.append(input);
  input.select();
  const copied = document.execCommand("copy");
  input.remove();
  if (!copied) throw new Error("Unable to copy path to the clipboard");
}

export async function openInFileManager(path: string): Promise<void> {
  if (!isTauri()) return;
  await invoke("open_in_file_manager", { path });
}

export async function getExternalAppSettings(): Promise<ExternalAppSettings> {
  if (isTauri()) return invoke("get_external_app_settings");
  const saved = localStorage.getItem("oxyviewer.demo.externalApps");
  return saved ? JSON.parse(saved) : { apps: [], defaultAppId: null };
}

export async function updateExternalAppSettings(settings: ExternalAppSettings): Promise<ExternalAppSettings> {
  if (isTauri()) return invoke("update_external_app_settings", { settings });
  localStorage.setItem("oxyviewer.demo.externalApps", JSON.stringify(settings));
  return settings;
}

export async function chooseExternalApplication(): Promise<string | null> {
  if (!isTauri()) return null;
  const selected = await open({ multiple: false, directory: false, filters: navigator.userAgent.includes("Windows")
    ? [{ name: "Application", extensions: ["exe"] }] : undefined });
  return typeof selected === "string" ? selected : null;
}

export async function openAssetWithApplication(path: string, appId: string): Promise<ExternalOpenResult> {
  if (!isTauri()) throw new Error("Opening external applications requires the desktop app");
  return invoke("open_asset_with_application", { path, appId });
}

export async function openAssetWithSystemDialog(path: string): Promise<ExternalOpenResult> {
  if (!isTauri()) throw new Error("Opening external applications requires the desktop app");
  return invoke("open_asset_with_system_dialog", { path });
}

export async function getAssetDetails(asset: AssetSummary): Promise<AssetDetails> {
  if (!isTauri()) {
    return {
      asset,
      width: 6_240,
      height: 4_160,
      metadata: {
        rating: asset.rating,
        colorLabel: asset.colorLabel,
        pickLabel: asset.pickLabel,
        creator: "OxyViewer Demo",
        copyright: "Personal archive",
        keywords: ["field-notes", asset.kind],
        hierarchicalKeywords: [],
      },
      embeddedKeywords: asset.kind === "heif" ? ["embedded-demo"] : [],
      embeddedHierarchicalKeywords: [],
      metadataCapability: {
        provider: asset.kind === "raw" ? "sidecar" : "native",
        readable: true,
        writable: true,
      },
      sidecarPath: asset.hasSidecar ? asset.path.replace(/\.[^.]+$/, ".xmp") : undefined,
      captureMetadata: {
        aperture: "f/2.8",
        exposureTime: "1/250 s",
        focalLength: "50 mm",
        iso: "100",
        exposureCompensation: "+0.3 EV",
        capturedAt: "2026:08:24 17:42:18",
        cameraMake: "Sony",
        cameraModel: "ILCE-7RM5",
        lensMake: "Sony",
        lensModel: "FE 50mm F1.4 GM",
        chromaSubsampling: asset.kind === "heif" ? "4:2:0" : "4:2:2",
        colorTemperature: "Auto",
        tint: "0",
        dynamicRangeOptimizer: "Auto",
      },
      focusInfo: ["raw", "heif", "jpeg"].includes(asset.kind) ? {
        coordinateWidth: 6_240,
        coordinateHeight: 4_160,
        regions: [{
          centerX: 1_400 + (Number(asset.id.replace("demo-", "")) % 4) * 1_150,
          centerY: 1_350 + (Number(asset.id.replace("demo-", "")) % 3) * 620,
          width: asset.kind === "raw" ? undefined : 240,
          height: asset.kind === "raw" ? undefined : 240,
        }],
      } : undefined,
    };
  }
  const result = await invoke<AssetDetailsResult>("get_asset_details", { path: asset.path });
  acceptMetadataProjection(result.metadataProjection);
  return result.details;
}

export async function requestMetadata(
  paths: string[],
  priority: MetadataRequestPriority,
): Promise<MetadataProjection[]> {
  if (!isTauri()) {
    return demoAssets.filter((asset) => paths.includes(asset.path)).map((asset, index) => ({
      path: asset.path,
      sourceRevision: `${asset.modifiedAtMs}:${asset.sizeBytes}:${asset.hasSidecar ? 1 : 0}`,
      stateRevision: Date.now() + index,
      validAt: Date.now() + index,
      status: "ready",
      rating: asset.rating,
      colorLabel: asset.colorLabel,
      pickLabel: asset.pickLabel,
    }));
  }
  return invoke<MetadataProjection[]>("request_metadata", { paths, priority });
}

export async function onMetadataProjectionUpdated(
  callback: (projection: MetadataProjection) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<MetadataProjection>("metadata-projection-updated", (event) => callback(event.payload));
}

export async function onImageProjectionUpdated(
  callback: (projection: ImageProjection) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<ImageProjection>("image-projection-updated", (event) => callback(event.payload));
}

export async function patchMetadata(paths: string[], patch: MetadataPatch): Promise<string> {
  if (!isTauri()) {
    // Replace (not mutate) entries so React Query structural sharing sees the
    // change and re-renders, matching the fresh projections Tauri pushes.
    for (let index = 0; index < demoAssets.length; index += 1) {
      const asset = demoAssets[index];
      if (!paths.includes(asset.path)) continue;
      demoAssets[index] = {
        ...asset,
        ...("rating" in patch ? { rating: patch.rating ?? undefined } : {}),
        ...("colorLabel" in patch ? { colorLabel: patch.colorLabel ?? undefined } : {}),
        ...("pickLabel" in patch ? { pickLabel: patch.pickLabel ?? undefined } : {}),
        hasSidecar: true,
      };
    }
    return "demo-metadata-job";
  }
  return invoke<string>("patch_metadata", { paths, patch });
}

function refreshDemoTagPaths(): void {
  const byId = new Map(demoTags.map((tag) => [tag.id, tag]));
  const pathFor = (tag: CustomTag): string => {
    const parent = tag.parentId === undefined ? undefined : byId.get(tag.parentId);
    return parent ? `${pathFor(parent)}|${tag.name}` : tag.name;
  };
  demoTags = demoTags.map((tag) => ({ ...tag, path: pathFor(tag) }));
}

export async function listCustomTags(): Promise<CustomTag[]> {
  if (!isTauri()) return demoTags.map((tag) => ({ ...tag }));
  return (await invoke<CustomTag[]>("list_custom_tags")).map(normalizeCustomTag);
}

export async function getAssetTagAssignments(paths: string[]): Promise<AssetTagAssignment[]> {
  if (!isTauri()) {
    return demoTags.map((tag) => ({
      tag: { ...tag },
      assignedCount: paths.filter((path) => demoAssetTags.get(path)?.has(tag.id)).length,
      assetCount: paths.length,
    }));
  }
  return (await invoke<AssetTagAssignment[]>("get_asset_tag_assignments", { paths }))
    .map((assignment) => ({ ...assignment, tag: normalizeCustomTag(assignment.tag) }));
}

export async function getAssetTagAssignmentsByPath(paths: string[]): Promise<AssetTagAssignmentsByPath[]> {
  if (!isTauri()) return Promise.all(paths.map(async (path) => ({
    path,
    assignments: (await getAssetTagAssignments([path])).filter((assignment) => assignment.assignedCount > 0),
  })));
  return (await invoke<AssetTagAssignmentsByPath[]>("get_asset_tag_assignments_by_path", { paths }))
    .map((entry) => ({ ...entry, assignments: entry.assignments.map((assignment) => ({
      ...assignment, tag: normalizeCustomTag(assignment.tag),
    })) }));
}

export async function createCustomTag(parentId: number | undefined, name: string): Promise<CustomTag> {
  if (!isTauri()) {
    const siblings = demoTags.filter((tag) => tag.parentId === parentId);
    const tag: CustomTag = { id: demoTagId++, parentId, name: name.trim(), path: "", sortOrder: siblings.length };
    demoTags.push(tag);
    refreshDemoTagPaths();
    return { ...demoTags.find((item) => item.id === tag.id)! };
  }
  return normalizeCustomTag(await invoke<CustomTag>("create_custom_tag", { parentId: parentId ?? null, name }));
}

export async function updateCustomTag(
  id: number,
  parentId: number | undefined,
  name: string,
): Promise<CustomTag> {
  if (!isTauri()) {
    demoTags = demoTags.map((tag) => tag.id === id ? { ...tag, parentId, name: name.trim() } : tag);
    refreshDemoTagPaths();
    return { ...demoTags.find((tag) => tag.id === id)! };
  }
  return normalizeCustomTag(await invoke<CustomTag>("update_custom_tag", { id, parentId: parentId ?? null, name }));
}

export async function getCustomTagDeleteImpact(id: number): Promise<TagDeleteImpact> {
  if (!isTauri()) {
    const descendants = new Set([id]);
    for (let changed = true; changed;) {
      changed = false;
      for (const tag of demoTags) {
        if (tag.parentId !== undefined && descendants.has(tag.parentId) && !descendants.has(tag.id)) {
          descendants.add(tag.id);
          changed = true;
        }
      }
    }
    const assetCount = [...demoAssetTags.values()].filter((ids) => [...descendants].some((tagId) => ids.has(tagId))).length;
    return { tagCount: descendants.size, assetCount };
  }
  return invoke<TagDeleteImpact>("get_custom_tag_delete_impact", { id });
}

export async function deleteCustomTag(id: number): Promise<TagDeleteImpact> {
  if (!isTauri()) {
    const impact = await getCustomTagDeleteImpact(id);
    const remove = new Set<number>();
    const collect = (tagId: number) => {
      remove.add(tagId);
      demoTags.filter((tag) => tag.parentId === tagId).forEach((tag) => collect(tag.id));
    };
    collect(id);
    demoTags = demoTags.filter((tag) => !remove.has(tag.id));
    for (const ids of demoAssetTags.values()) for (const tagId of remove) ids.delete(tagId);
    return impact;
  }
  return invoke<TagDeleteImpact>("delete_custom_tag", { id });
}

export async function setAssetCustomTag(paths: string[], tagId: number, assigned: boolean): Promise<void> {
  if (!isTauri()) {
    for (const path of paths) {
      const ids = demoAssetTags.get(path) ?? new Set<number>();
      if (assigned) { ids.add(tagId); demoFaceOwnedTags.get(path)?.delete(tagId); }
      else ids.delete(tagId);
      demoAssetTags.set(path, ids);
    }
    return;
  }
  await invoke("set_asset_custom_tag", { paths, tagId, assigned });
}

export async function getTagSyncStatus(): Promise<TagSyncStatus> {
  if (!isTauri()) return { pendingCount: 0, failedCount: 0 };
  return invoke<TagSyncStatus>("get_tag_sync_status");
}

export async function retryTagXmpSync(): Promise<void> {
  if (!isTauri()) return;
  await invoke("retry_tag_xmp_sync");
}

export async function syncMetadataToEmbedded(paths: string[]): Promise<string> {
  if (!isTauri()) return "demo-embedded-sync-job";
  return invoke<string>("sync_metadata_to_embedded", { paths });
}

export async function getExiftoolStatus(): Promise<ExiftoolStatus> {
  if (!isTauri()) return { available: true, source: "path", version: "demo" };
  return invoke<ExiftoolStatus>("get_exiftool_status");
}

export async function installExiftool(): Promise<ExiftoolStatus> {
  if (!isTauri()) return { available: true, source: "managed", version: "demo" };
  return invoke<ExiftoolStatus>("install_exiftool");
}

export async function chooseAndConfigureExiftool(): Promise<ExiftoolStatus | null> {
  if (!isTauri()) return { available: true, source: "user", version: "demo" };
  const selection = await open({
    directory: false,
    multiple: false,
    title: "Select ExifTool executable",
  });
  if (typeof selection !== "string") return null;
  return invoke<ExiftoolStatus>("configure_exiftool", { path: selection });
}

const demoRoots: string[] = [];

export async function addLibraryRoot(path: string): Promise<string[]> {
  if (!isTauri()) {
    if (!demoRoots.includes(path)) demoRoots.push(path);
    return [...demoRoots];
  }
  return invoke<string[]>("add_library_root", { path });
}

export async function removeLibraryRoot(path: string): Promise<string[]> {
  if (!isTauri()) {
    const index = demoRoots.indexOf(path);
    if (index >= 0) demoRoots.splice(index, 1);
    return [...demoRoots];
  }
  return invoke<string[]>("remove_library_root", { path });
}

export async function listLibraryRoots(): Promise<string[]> {
  if (!isTauri()) return [...demoRoots];
  return invoke<string[]>("list_library_roots");
}

export async function reorderLibraryRoots(paths: string[]): Promise<string[]> {
  if (!isTauri()) {
    if (paths.length !== demoRoots.length || paths.some((path) => !demoRoots.includes(path))) {
      throw new Error("Folder order must contain every library root exactly once");
    }
    demoRoots.splice(0, demoRoots.length, ...paths);
    return [...demoRoots];
  }
  return invoke<string[]>("reorder_library_roots", { paths });
}

/**
 * Metadata mirrored from the desktop bundle for the browser demo, which has no
 * backend to report it. `__OXY_APP_VERSION__` comes from apps/desktop/package.json,
 * so the demo version cannot drift from the shipped one.
 */
const demoAppInfo: AppInfo = {
  name: "OxyViewer",
  version: __OXY_APP_VERSION__,
  repositoryUrl: "https://github.com/oxygenkun/oxyviewer",
  author: "oxygenkun",
  license: "AGPL-3.0-only OR LicenseRef-OxyViewer-Commercial",
};

let demoCacheSettings: CacheSettings = {
  location: "/demo/OxyViewer Cache/previews",
  defaultLocation: "/demo/OxyViewer Cache/previews",
  customParent: undefined,
  isCustomLocation: false,
  maxSizeBytes: 10 * 1024 ** 3,
  usedSizeBytes: 2.4 * 1024 ** 3,
};

export async function getCacheSettings(): Promise<CacheSettings> {
  if (!isTauri()) return demoCacheSettings;
  return invoke<CacheSettings>("get_cache_settings");
}

export async function chooseCacheParent(): Promise<string | null> {
  if (!isTauri()) return "/demo/Fast SSD";
  const selection = await open({
    directory: true,
    multiple: false,
    title: "Choose cache location",
  });
  return typeof selection === "string" ? selection : null;
}

export async function updateCacheSettings(
  customParent: string | null,
  maxSizeBytes: number,
): Promise<CacheSettings> {
  if (!isTauri()) {
    demoCacheSettings = {
      ...demoCacheSettings,
      location: customParent
        ? `${customParent}/OxyViewer Cache/previews`
        : demoCacheSettings.defaultLocation,
      customParent: customParent ?? undefined,
      isCustomLocation: Boolean(customParent),
      maxSizeBytes,
      usedSizeBytes: Math.min(demoCacheSettings.usedSizeBytes, maxSizeBytes),
    };
    return demoCacheSettings;
  }
  return invoke<CacheSettings>("update_cache_settings", {
    update: { customParent, maxSizeBytes },
  });
}

export async function clearPreviewCache(): Promise<CacheSettings> {
  if (!isTauri()) {
    demoCacheSettings = { ...demoCacheSettings, usedSizeBytes: 0 };
    return demoCacheSettings;
  }
  return invoke<CacheSettings>("clear_preview_cache");
}

export async function getDebugQueueSnapshot(): Promise<DebugQueueSnapshot> {
  if (!__OXY_DEBUG__) throw new Error("Queue diagnostics require a debug build");
  if (!isTauri()) {
    return { capturedAtUnixMs: Date.now(), workerWaitMicros: 0, collectionMicros: 0, staleQueues: [], queues: [] };
  }
  return invoke<DebugQueueSnapshot>("get_debug_queue_snapshot");
}

export async function openDebugQueueWindow(): Promise<void> {
  if (!__OXY_DEBUG__) return;
  if (!isTauri()) {
    window.open("?debug=queues", "oxyviewer-debug-queues", "popup,width=1180,height=760");
    return;
  }
  await invoke("open_debug_queue_window");
}

export async function closeDebugQueueWindow(): Promise<void> {
  if (!isTauri()) {
    window.close();
    return;
  }
  await invoke("close_debug_queue_window");
}

export function previewUrl(asset: AssetSummary): string | undefined {
  // Native raster files use the same registered resource boundary as every
  // other format. Browser demo assets remain ordinary browser URLs.
  return isTauri() ? undefined : asset.path;
}

export async function generatedPreview(
  asset: AssetSummary,
  level: RenderLevel,
  signal?: AbortSignal,
  priority: PreviewPriority = "visible",
  rank = 0,
): Promise<PreviewResult | undefined> {
  if (!isTauri()) return undefined;
  if (level === "thumbnail") {
    return sharedThumbnailRequests.request(asset, signal, priority, rank, (sharedSignal, sharedPriority, sharedRank) =>
      requestGeneratedPreview(asset, level, sharedSignal, sharedPriority, sharedRank));
  }
  return requestGeneratedPreview(asset, level, signal, priority, rank);
}

/** Encoded, bounded whole-photo preview for analysis cards. The host validates
 * the source revision; callers do not invent file metadata or request Full. */
export async function faceWorkbenchPreview(path: string, signal?: AbortSignal): Promise<PreviewResult | undefined> {
  if (!isTauri()) {
    ensureDemoFaces();
    const faces = demoFaceReview.items.filter((face) => face.assetPath === path);
    const shapes = faces.map(({ bbox }) => `<ellipse cx="${(bbox.x + bbox.width / 2) * 600}" cy="${(bbox.y + bbox.height / 2) * 400}" rx="${bbox.width * 280}" ry="${bbox.height * 190}" fill="#8c9298"/>`).join("");
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="600" height="400"><rect width="600" height="400" fill="#262a30"/>${shapes}<text x="24" y="370" fill="#8c9298" font-family="sans-serif" font-size="16">DEMO</text></svg>`;
    return { path, url: `data:image/svg+xml,${encodeURIComponent(svg)}`, width: 600, height: 400, kind: "decoded", renderLevel: "thumbnail" };
  }
  return requestGeneratedPreview({ path, name: path.split(/[\\/]/).at(-1) ?? path }, "thumbnail", signal, "visible", 0);
}

async function requestGeneratedPreview(
  asset: Pick<AssetSummary, "path" | "name">,
  level: RenderLevel,
  signal: AbortSignal | undefined,
  priority: PreviewPriority,
  rank: number,
): Promise<PreviewResult | undefined> {
  // Do not register a debug WAIT entry for work React Query has already
  // cancelled. There is no lifecycle handle to clean up if we throw first.
  if (signal?.aborted) throw signal.reason ?? new DOMException("Aborted", "AbortError");
  const debug = __OXY_DEBUG__
      ? beginPreviewDebug({
          assetName: asset.name,
          stage: level,
          priority,
          resourceKey: `preview:${asset.path}:${level}`,
          resourceLabel: level,
        })
    : undefined;
  debug?.start();
  perfMark("preview:queued", { assetName: asset.name, level, priority });
  const requestId = crypto.randomUUID();
  const backendRequest = invoke<ImageProjection>("get_preview", {
    requestId,
    path: asset.path,
    level,
    priority,
    rank,
  }).then((projection) => {
    if (signal?.aborted && projection.result?.resource) {
      releaseUnretainedMediaResource(projection.result.resource.resourceId);
    }
    return projection;
  });
  let rejectAbort: ((reason: unknown) => void) | undefined;
  const aborted = new Promise<never>((_resolve, reject) => {
    rejectAbort = reject;
  });
  const stopWaiting = () => {
    cancelGeneratedPreviewRequest(asset, level, requestId);
    rejectAbort?.(signal?.reason ?? new DOMException("Aborted", "AbortError"));
  };
  signal?.addEventListener("abort", stopWaiting, { once: true });
  let interim = false;
  try {
    const projection = await (signal ? Promise.race([backendRequest, aborted]) : backendRequest);
    if (signal?.aborted) throw signal.reason ?? new DOMException("Aborted", "AbortError");
    if (!acceptImageProjection(projection)) {
      debug?.cancel();
      return undefined;
    }
    const result = projection.result;
    if (!result) throw new Error("Ready image projection has no artifact");
    perfMark("preview:result", {
      assetName: asset.name,
      level,
      priority,
      width: result.width,
      height: result.height,
      kind: result.kind,
      renderLevel: result.renderLevel,
      diagnostics: result.diagnostics,
    });
    debug?.mark("backend-result", {
      width: result.width,
      height: result.height,
      kind: result.kind,
      renderLevel: result.renderLevel,
      diagnostics: result.diagnostics,
    });
    debug?.complete({ diagnostics: result.diagnostics });
    if (!result.resource) throw new Error("Ready image projection has no registered resource");
    // Interim is a displayable query result, not a promise to hide behind.
    // Rust keeps the native request alive through its upgrade and publishes
    // either Satisfied or terminal Error into the projection store. Settling
    // here also lets independent full-detail renderers (HEIF tiles) start.
    interim = level !== "thumbnail" && result.satisfaction === "interim";
    return { ...result, url: mediaProtocolUrl(result.resource.url) };
  } catch (error) {
    if (signal?.aborted) debug?.cancel();
    else debug?.fail(error);
    throw error;
  } finally {
    if (!interim) signal?.removeEventListener("abort", stopWaiting);
  }
}

export async function renewMediaResource(resourceId: string): Promise<boolean> {
  if (!isTauri()) return true;
  return invoke<boolean>("renew_media_resource", { resourceId });
}

export async function releaseMediaResource(resourceId: string): Promise<void> {
  if (!isTauri()) return;
  await invoke("release_media_resource", { resourceId });
}

export async function reconcilePreviewSchedule(
  scopeId: string,
  epoch: number,
  intents: PreviewScheduleIntent[],
  omittedPolicy: PreviewOmittedPolicy,
): Promise<boolean> {
  if (!isTauri()) return true;
  return invoke<boolean>("reconcile_preview_schedule", {
    scopeId,
    epoch,
    intents,
    omittedPolicy,
  });
}

export async function upsertPreviewSchedule(
  scopeId: string,
  epoch: number,
  intent: Omit<PreviewScheduleIntent, "rank">,
  placement: SchedulePlacement,
): Promise<boolean> {
  if (!isTauri()) return true;
  return invoke<boolean>("upsert_preview_schedule", {
    scopeId,
    epoch,
    path: intent.path,
    level: intent.level,
    priority: intent.priority,
    placement,
  });
}

export async function releasePreviewSchedule(
  scopeId: string,
  epoch: number,
  path: string,
  level: RenderLevel,
): Promise<boolean> {
  if (!isTauri()) return true;
  return invoke<boolean>("release_preview_schedule", { scopeId, epoch, path, level });
}

/**
 * Requests real thumbnail work and retains a small, native-resource-independent
 * browser copy for the entire current directory. Nearby callers can jump ahead.
 */
export async function preloadAssetThumbnail(
  asset: AssetSummary,
  signal?: AbortSignal,
  priority: PreviewPriority = "preload",
  rank = 0,
): Promise<void> {
  signal?.throwIfAborted();
  if (getFolderThumbnail(asset)) return;
  const generation = getFolderThumbnailGeneration();
  const directSource = previewUrl(asset);
  if (directSource) {
    await browserPreloadQueue.enqueue(
      orderedPriorityWeight(priority, rank),
      signal,
      () => preloadFolderThumbnail(asset, directSource, undefined, signal, generation),
    );
    return;
  }
  const result = await generatedPreview(asset, "thumbnail", signal, priority, rank);
  if (result) {
    const id = result.resource?.resourceId;
    const release = id ? retainMediaResource(id) : undefined;
    try {
      await browserPreloadQueue.enqueue(
        orderedPriorityWeight(priority, rank),
        signal,
        async () => {
          signal?.throwIfAborted();
          if (generation !== getFolderThumbnailGeneration()) return;
          if (id && !await renewMediaResource(id)) throw new Error("Thumbnail resource expired before preload");
          await preloadFolderThumbnail(asset, result.url, result.geometry, signal, generation);
        },
      );
    } finally { release?.(); }
  }
}

export async function startHeifFull(
  path: string,
  generation: number,
  displaySharpening: boolean,
  signal?: AbortSignal,
): Promise<
  | { delivery: "artifact"; projection: ImageProjection; result: PreviewResult }
  | Extract<HeifFullPresentation, { delivery: "tiles" }>
> {
  signal?.throwIfAborted();
  perfMark("heif:decode-requested", { path });
  const requestId = crypto.randomUUID();
  const cancel = () => {
    void invoke("cancel_preview_request", { path, level: "full", requestId }).catch(() => {});
  };
  signal?.addEventListener("abort", cancel, { once: true });
  try {
    const presentation = await invoke<HeifFullPresentation>("start_heif_full", {
      requestId, path, generation, displaySharpening,
    });
    if (signal?.aborted) {
      if (presentation.delivery === "artifact") {
        const id = presentation.projection.result?.resource?.resourceId;
        if (id) releaseUnretainedMediaResource(id);
      } else {
        await cancelHeifDecode(presentation.session.id);
      }
      signal.throwIfAborted();
    }
    if (presentation.delivery === "artifact") {
      const result = presentation.projection.result;
      if (!result) throw new Error("ready HEIF full projection has no artifact");
      if (!result.resource) throw new Error("Ready HEIF artifact has no registered resource");
      return {
        ...presentation,
        result: { ...result, url: mediaProtocolUrl(result.resource.url) },
      };
    }
    perfMark("heif:decode-session", { path, sessionId: presentation.session.id });
    return presentation;
  } finally {
    signal?.removeEventListener("abort", cancel);
  }
}

export async function cancelHeifDecode(sessionId: string): Promise<boolean> {
  return invoke<boolean>("cancel_heif_decode", { sessionId });
}

export async function getHeifCapabilities(): Promise<HeifCapabilities[]> {
  if (!isTauri()) return [];
  return invoke<HeifCapabilities[]>("get_heif_capabilities");
}

export async function getRawDecoderStatus(path?: string, refresh = false): Promise<RawDecoderStatus> {
  if (!isTauri()) return { installAvailable: false, availability: "unsupportedPlatform", codecs: [], detail: null, attempt: null };
  return invoke<RawDecoderStatus>("get_raw_decoder_status", { path: path ?? null, refresh });
}

export async function retryRawFull(path: string): Promise<void> {
  if (isTauri()) await invoke("retry_raw_full", { path });
}

export async function openRawDecoderInstallPage(web = false): Promise<"store" | "web"> {
  if (!isTauri()) {
    window.open("https://apps.microsoft.com/detail/9nctdw2w1bh8", "_blank", "noopener,noreferrer");
    return "web";
  }
  return invoke("open_raw_decoder_install_page", { web });
}

export function heifTileUrl(url: string): string {
  return mediaProtocolUrl(url);
}

/** Returns the E2E performance scenario injected by the runner, if any. */
export async function getPerfScenario(): Promise<PerfScenario | undefined> {
  if (!isTauri()) return undefined;
  return (await invoke<PerfScenario | null>("get_perf_scenario")) ?? undefined;
}

/** Persists the performance report JSON via the backend. */
export async function writePerfReport(path: string, report: unknown): Promise<void> {
  if (!isTauri()) {
    console.info("[OxyPerf] report", report);
    return;
  }
  await invoke("write_perf_report", { path, contents: JSON.stringify(report, null, 2) });
}

/** Explicit native resource diagnostics for debug/performance harnesses. */
export async function getMediaResourceStats(): Promise<import("@/types").ResourceRegistryStats | undefined> {
  if (!isTauri()) return undefined;
  return invoke("get_media_resource_stats");
}

/**
 * Build metadata for the About panel.
 *
 * The desktop build reads the version from the running package. The browser demo
 * has no backend, so it reports the version injected at build time.
 */
export async function getAppInfo(): Promise<AppInfo> {
  if (!isTauri()) return demoAppInfo;
  return invoke<AppInfo>("get_app_info");
}

/**
 * Queries GitHub Releases for the newest published release.
 *
 * Only user-initiated: the app never polls for updates in the background, and it
 * never downloads or installs one from this path.
 */
export async function checkForUpdates(): Promise<UpdateStatus> {
  if (!isTauri()) {
    await new Promise((resolve) => window.setTimeout(resolve, 400));
    return {
      currentVersion: demoAppInfo.version,
      latestVersion: demoAppInfo.version,
      updateAvailable: false,
      releaseUrl: `${demoAppInfo.repositoryUrl}/releases`,
    };
  }
  return invoke<UpdateStatus>("check_for_updates");
}

/**
 * Opens one of the About panel's fixed destinations in the system browser.
 * `releaseUrl` is accepted only when it points at an OxyViewer release page.
 */
export async function openAboutLink(target: AboutLink, releaseUrl?: string): Promise<void> {
  if (!isTauri()) {
    const destinations: Record<AboutLink, string> = {
      repository: demoAppInfo.repositoryUrl,
      releases: `${demoAppInfo.repositoryUrl}/releases`,
      license: `${demoAppInfo.repositoryUrl}/blob/main/LICENSE.md`,
    };
    window.open(destinations[target], "_blank", "noopener,noreferrer");
    return;
  }
  await invoke("open_about_link", { target, releaseUrl: releaseUrl ?? null });
}

/* ---------------------------------------------------------------------------
 * Face analysis and people.
 *
 * The browser demo keeps an in-memory model so the review flow can be exercised
 * without the native analyzer; the desktop path always goes through the Host so
 * machine output and user decisions stay separated.
 * ------------------------------------------------------------------------- */

const demoFacePersons: Person[] = [];

let demoFaceClusters: FaceCluster[] = [];
let demoFaceReview: FaceReviewPage = { items: [], total: 0, nextCursor: null };
let demoFaceStats: FaceLibraryStats = {
  analyzedAssets: 0,
  facesDetected: 0,
  persons: 0,
  pendingReviews: 0,
  unknownFaces: 0,
  clusters: 0,
};
let demoFaceSettings: FaceAnalyzerSettings = {
  detectionConfidence: 0.5,
  nmsThreshold: 0.3,
  maxFacesPerAsset: 64,
  minFacePixels: 24,
  detectSmallFaces: false,
  matchSensitivity: "balanced",
  matchThreshold: 0.363,
  clusterThreshold: 0.363,
};

function demoFaceItem(index: number, state: FaceReviewPage["items"][number]["state"]): FaceReviewPage["items"][number] {
  const seed = index + 1;
  const width = 0.18 + (seed % 3) * 0.02;
  return {
    observationId: `demo-face-${index}`,
    assetId: `demo-${index}`,
    assetPath: `/demo/portrait-${index}.jpg`,
    bbox: { x: 0.1 + (seed % 4) * 0.12, y: 0.12, width, height: width * 1.3 },
    detectionScore: 0.97,
    facePixels: 48 + index * 24,
    clarity: 0.35 + index * 0.12,
    state,
  };
}

/** Builds the demo review queue once, so the people panel has something to show. */
let demoFacesInitialized = false;
function ensureDemoFaces(): void {
  if (demoFacesInitialized) return;
  demoFacesInitialized = true;
  if (!demoFacePersons.some((person) => person.personId === "demo-person-1")) demoFacePersons.push({ personId: "demo-person-1", displayName: "Demo Person", createdAtMs: 0, updatedAtMs: 0, faceCount: 0 });
  const states: Array<FaceReviewPage["items"][number]["state"]> = [
    "pending",
    "pending",
    "unknown",
    "unknown",
    "unknown",
  ];
  const items = states.map((state, index) => {
    const item = demoFaceItem(index, state);
    if (state === "pending") {
      item.candidate = {
        observationId: item.observationId,
        personId: "demo-person-1",
        personName: "Demo Person",
        similarity: 0.52 - index * 0.05,
        matcherFingerprint: "demo/matcher",
      };
    }
    return item;
  });
  demoFaceReview = { items, total: items.length, nextCursor: null };
  demoFaceClusters = [
    {
      clusterId: "demo-cluster-1",
      observationIds: ["demo-face-2", "demo-face-3", "demo-face-4"],
      representativeObservationId: "demo-face-2",
      memberCount: 3,
      cohesion: 0.71,
      outlierObservationIds: ["demo-face-4"],
      memberQuality: items.slice(2).map((item) => ({
        observationId: item.observationId,
        detectionScore: item.detectionScore,
        facePixels: item.facePixels ?? 0,
        clarity: item.clarity ?? 0,
      })),
    },
  ];
  demoFaceStats = {
    analyzedAssets: 5,
    facesDetected: items.length,
    persons: demoFacePersons.length,
    pendingReviews: items.filter((item) => item.state === "pending").length,
    unknownFaces: items.filter((item) => item.state === "unknown").length,
    clusters: demoFaceClusters.length,
  };
}

export async function getFaceCapability(): Promise<FaceCapability> {
  if (!isTauri()) {
    ensureDemoFaces();
    return {
      available: true,
      running: false,
      settings: demoFaceSettings,
      stats: demoFaceStats,
      models: [
        { id: "scrfd-10g-kps", displayName: "SCRFD-10G KPS", installed: false, sizeBytes: 16_923_827, downloadSizeBytes: 288_621_354, licenseSummary: "Non-commercial research only." },
        { id: "adaface-ir101", displayName: "AdaFace IR-101", installed: false, sizeBytes: 260_704_652, downloadSizeBytes: 260_704_652, licenseSummary: "Experimental model licensing must be reviewed." },
      ],
      peopleStorePath: "demo/people.json",
    };
  }
  return invoke<FaceCapability>("get_face_capability");
}

export async function installFaceModel(modelId: string): Promise<FaceCapability> {
  if (!isTauri()) {
    const capability = await getFaceCapability();
    capability.models = capability.models.map((model) => model.id === modelId ? { ...model, installed: true } : model);
    capability.available = capability.models.every((model) => model.installed);
    return capability;
  }
  return invoke<FaceCapability>("install_face_model", { modelId });
}

/**
 * Accept/reject score distributions from real decisions, plus a suggested
 * threshold. `ox-faces` computes the recommendation from this library's data.
 */
export async function getFaceCalibration(): Promise<FaceCalibration> {
  if (!isTauri()) {
    return {
      acceptedScores: [],
      rejectedScores: [],
      currentThreshold: demoFaceSettings.matchThreshold,
      separable: false,
    };
  }
  return invoke<FaceCalibration>("get_face_calibration");
}

export async function updateFaceAnalyzerSettings(
  settings: FaceAnalyzerSettings,
): Promise<FaceAnalyzerSettings> {
  if (!isTauri()) {
    demoFaceSettings = { ...settings };
    return demoFaceSettings;
  }
  return invoke<FaceAnalyzerSettings>("update_face_analyzer_settings", { settings });
}

export async function startFaceAnalysis(request: FaceAnalysisRequest = {}): Promise<string> {
  if (!isTauri()) {
    demoFacesInitialized = false;
    ensureDemoFaces();
    return "demo-face-job";
  }
  return invoke<string>("start_face_analysis", {
    request: {
      paths: request.paths ?? [],
      rootPath: request.rootPath ?? null,
      directory: request.directory ?? null,
      force: request.force ?? false,
    },
  });
}

export async function cancelFaceAnalysis(jobId: string): Promise<boolean> {
  if (!isTauri()) return true;
  return invoke<boolean>("cancel_face_analysis", { jobId });
}

export async function listPersons(): Promise<Person[]> {
  if (!isTauri()) {
    ensureDemoFaces();
    return [...demoFacePersons];
  }
  return invoke<Person[]>("list_persons");
}

export async function createPerson(
  personId: string,
  displayName: string,
  linkedTagId?: number | null,
): Promise<Person> {
  if (!isTauri()) {
    ensureDemoFaces();
    const tag = linkedTagId == null ? await ensureTagPath(`人物|${displayName}`, demoTags, createCustomTag) : demoTags.find((tag) => tag.id === linkedTagId);
    if (!tag) throw new Error("Tag not found");
    const person: Person = {
      personId,
      displayName,
      linkedTagId: tag.id,
      createdAtMs: Date.now(),
      updatedAtMs: Date.now(),
      faceCount: 0,
    };
    demoFacePersons.push(person);
    demoFaceStats = { ...demoFaceStats, persons: demoFacePersons.length };
    return person;
  }
  return invoke<Person>("create_person", {
    personId,
    displayName,
    linkedTagId: linkedTagId ?? null,
  });
}

export async function renamePerson(personId: string, displayName: string): Promise<void> {
  if (!isTauri()) {
    const person = demoFacePersons.find((entry) => entry.personId === personId);
    if (person) {
      const path = demoTags.find((tag) => tag.id === person.linkedTagId)?.path ?? `人物|${person.displayName}`;
      const parent = path.split("|").slice(0, -1).join("|") || "人物";
      await setPersonTagPath(personId, `${parent}|${displayName}`);
      person.displayName = displayName;
      for (const face of demoFaceReview.items) if (face.confirmedPersonId === personId) face.confirmedPersonName = displayName;
    }
    return;
  }
  await invoke("rename_person", { personId, displayName });
}

export async function linkPersonTag(personId: string, tagId: number | null): Promise<void> {
  if (!isTauri()) {
    const person = demoFacePersons.find((person) => person.personId === personId);
    if (!person) throw new Error("Person not found");
    const path = demoTags.find((tag) => tag.id === tagId)?.path ?? `人物|${person.displayName}`;
    await setPersonTagPath(personId, path);
    return;
  }
  await invoke("link_person_tag", { personId, tagId });
}

export async function setPersonTagPath(personId: string, path: string): Promise<void> {
  if (!isTauri()) {
    ensureDemoFaces();
    const person = demoFacePersons.find((person) => person.personId === personId);
    if (!person) throw new Error("Person not found");
    const tag = await ensureTagPath(path, demoTags, createCustomTag);
    person.linkedTagId = tag.id;
    person.updatedAtMs = Date.now();
    refreshDemoFaceCounts();
    return;
  }
  await invoke("set_person_tag_path", { personId, path });
}

/**
 * Folds one person into another. Every face confirmed for the source now
 * belongs to the target, so nothing has to be re-confirmed by hand.
 */
export async function mergePersons(
  sourcePersonId: string,
  targetPersonId: string,
): Promise<number> {
  if (!isTauri()) {
    const source = demoFacePersons.find((entry) => entry.personId === sourcePersonId);
    const target = demoFacePersons.find((entry) => entry.personId === targetPersonId);
    if (!source || !target) throw new Error("Unknown person");
    if (source === target) return 0;
    const moved = demoFaceReview.items.filter((face) => face.confirmedPersonId === sourcePersonId);
    if (target.linkedTagId === undefined) target.linkedTagId = (await ensureTagPath(`人物|${target.displayName}`, demoTags, createCustomTag)).id;
    for (const face of moved) {
      face.confirmedPersonId = targetPersonId;
      face.confirmedPersonName = target.displayName;
    }
    demoFacePersons.splice(demoFacePersons.indexOf(source), 1);
    refreshDemoFaceCounts();
    demoFaceStats = { ...demoFaceStats, persons: demoFacePersons.length };
    return moved.length;
  }
  return invoke<number>("merge_persons", { sourcePersonId, targetPersonId });
}

/** Detaches faces from a person without touching that person's other photos. */
export async function removeFacesFromPerson(
  personId: string,
  observationIds: string[],
): Promise<number> {
  if (!isTauri()) return 0;
  return invoke<number>("remove_faces_from_person", { personId, observationIds });
}

/** Confirms faces for a person, replacing any previous answer. */
export async function assignFacesToPerson(
  personId: string,
  observationIds: string[],
): Promise<number> {
  if (!isTauri()) {
    for (const id of observationIds) await decideFace(id, { decision: "confirmPerson", personId });
    return observationIds.length;
  }
  return invoke<number>("assign_faces_to_person", { personId, observationIds });
}

export async function getPersonUndo(): Promise<UndoableOperation | null> {
  if (!isTauri()) return null;
  return invoke<UndoableOperation | null>("get_person_undo");
}

export async function undoPersonOperation(): Promise<UndoableOperation | null> {
  if (!isTauri()) return null;
  return invoke<UndoableOperation | null>("undo_person_operation");
}

export async function deletePerson(personId: string): Promise<number> {
  if (!isTauri()) {
    const index = demoFacePersons.findIndex((entry) => entry.personId === personId);
    if (index >= 0) demoFacePersons.splice(index, 1);
    const affected = demoFaceReview.items.filter((face) => face.confirmedPersonId === personId);
    for (const face of affected) {
      face.state = "unknown";
      face.confirmedPersonId = undefined;
      face.confirmedPersonName = undefined;
    }
    refreshDemoFaceCounts();
    demoFaceStats = { ...demoFaceStats, persons: demoFacePersons.length };
    return affected.length;
  }
  return invoke<number>("delete_person", { personId });
}

function refreshDemoFaceCounts(): void {
  for (const [path, ids] of demoFaceOwnedTags) for (const id of ids) demoAssetTags.get(path)?.delete(id);
  demoFaceOwnedTags.clear();
  for (const face of demoFaceReview.items) {
    if (face.state !== "confirmed") continue;
    const person = demoFacePersons.find((person) => person.personId === face.confirmedPersonId);
    if (person?.linkedTagId === undefined) continue;
    const ids = demoAssetTags.get(face.assetPath) ?? new Set<number>();
    if (!ids.has(person.linkedTagId)) {
      const owned = demoFaceOwnedTags.get(face.assetPath) ?? new Set<number>();
      owned.add(person.linkedTagId);
      demoFaceOwnedTags.set(face.assetPath, owned);
    }
    ids.add(person.linkedTagId);
    demoAssetTags.set(face.assetPath, ids);
  }
  for (const person of demoFacePersons) person.faceCount = demoFaceReview.items.filter((item) => item.state === "confirmed" && item.confirmedPersonId === person.personId).length;
  demoFaceStats.pendingReviews = demoFaceReview.items.filter((item) => item.state === "pending").length;
  demoFaceStats.unknownFaces = demoFaceReview.items.filter((item) => item.state === "unknown").length;
}

export async function decideFace(
  observationId: string,
  decision: FaceDecision,
): Promise<void> {
  if (!isTauri()) {
    ensureDemoFaces();
    const item = demoFaceReview.items.find((entry) => entry.observationId === observationId);
    if (item) {
      item.state =
        decision.decision === "confirmPerson"
          ? "confirmed"
          : decision.decision === "rejectPerson"
            ? "rejected"
            : "notFace";
      item.confirmedPersonId =
        decision.decision === "confirmPerson" ? decision.personId : undefined;
      if (decision.decision === "confirmPerson") {
        const person = demoFacePersons.find((entry) => entry.personId === decision.personId);
        if (person && person.linkedTagId === undefined) person.linkedTagId = (await ensureTagPath(`人物|${person.displayName}`, demoTags, createCustomTag)).id;
        item.confirmedPersonName = person?.displayName;
      } else item.confirmedPersonName = undefined;
    }
    refreshDemoFaceCounts();
    return;
  }
  await invoke("decide_face", { observationId, decision });
}

/** Manual quality is independent of identity; null restores automatic judgement. */
export async function setFaceClarity(observationIds: string[], blurry: boolean | null): Promise<void> {
  if (!isTauri()) {
    ensureDemoFaces();
    const ids = new Set(observationIds);
    for (const item of demoFaceReview.items) {
      if (ids.has(item.observationId)) item.manualBlurry = blurry ?? undefined;
    }
    return;
  }
  await invoke("set_face_clarity", { observationIds, blurry });
}

export async function clearFaceDecision(observationId: string): Promise<void> {
  if (!isTauri()) {
    ensureDemoFaces();
    const item = demoFaceReview.items.find((entry) => entry.observationId === observationId);
    if (item) {
      item.state = item.candidate ? "pending" : "unknown";
      item.confirmedPersonId = undefined;
      item.confirmedPersonName = undefined;
    }
    refreshDemoFaceCounts();
    return;
  }
  await invoke("clear_face_decision", { observationId });
}

export async function getFaceReviewPage(
  cursor = 0,
  pageSize = 200,
): Promise<FaceReviewPage> {
  if (!isTauri()) {
    ensureDemoFaces();
    const items = demoFaceReview.items;
    return { items: structuredClone(items.slice(cursor, cursor + pageSize)), total: items.length, nextCursor: cursor + pageSize < items.length ? cursor + pageSize : null };
  }
  return invoke<FaceReviewPage>("get_face_review_page", { cursor, pageSize });
}

export async function getFaceClusters(): Promise<FaceCluster[]> {
  if (!isTauri()) {
    ensureDemoFaces();
    return [...demoFaceClusters];
  }
  return invoke<FaceCluster[]>("get_face_clusters");
}

/**
 * Crops for the review panel, keyed by observation.
 *
 * The backend returns `oxy-media://` resources rather than image bytes, so a
 * caller must lease each returned resource with `retainMediaResource` and
 * release it when the crop is no longer displayed.
 */
export async function getFaceCrops(
  observationIds: string[],
  size = 128,
): Promise<FaceCropResource[]> {
  if (!isTauri() || observationIds.length === 0) return [];
  return invoke<FaceCropResource[]>("get_face_crops", { observationIds, size });
}

/**
 * Face boxes for one asset with the user's current answer, so the loupe can
 * draw and correct them in place.
 */
export async function getAssetFaceReviews(path: string): Promise<FaceReviewItem[]> {
  if (!isTauri()) return [];
  return invoke<FaceReviewItem[]>("get_asset_face_reviews", { path });
}

export async function getAssetFaceObservations(path: string): Promise<FaceObservation[]> {
  if (!isTauri()) return [];
  return invoke<FaceObservation[]>("get_asset_face_observations", { path });
}

export async function onFaceAnalysisProgress(
  callback: (progress: FaceAnalysisProgress) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<FaceAnalysisProgress>("face-analysis-progress", (event) => callback(event.payload));
}

export async function onFaceModelDownloadProgress(
  callback: (progress: FaceModelDownloadProgress) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<FaceModelDownloadProgress>("face-model-download-progress", (event) => callback(event.payload));
}

export async function onFaceLibraryUpdated(
  callback: (stats: FaceLibraryStats) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<FaceLibraryStats>("face-library-updated", (event) => callback(event.payload));
}

/**
 * The asset one face belongs to, so clicking a crop can reveal the photo.
 *
 * `null` is an ordinary answer: a re-analysis can retire the observation a
 * click refers to, and the caller should say so rather than fail.
 */
export async function resolveFaceObservation(
  observationId: string,
): Promise<FaceAssetReveal | null> {
  if (!isTauri()) {
    ensureDemoFaces();
    const item = demoFaceReview.items.find((entry) => entry.observationId === observationId);
    return item
      ? { observationId: item.observationId, assetId: item.assetId, assetPath: item.assetPath }
      : null;
  }
  return invoke<FaceAssetReveal | null>("resolve_face_observation", { observationId });
}

// The face workbench runs in its own window. Tauri carries the two messages it
// needs (browse context out, "show me this photo" back) as events; the browser
// demo falls back to a BroadcastChannel so the same component works there.

const FACE_WORKBENCH_CHANNEL = "oxyviewer-face-workbench";

type FaceWorkbenchMessage =
  | { kind: "visibility"; visible: boolean }
  | { kind: "context"; context: FaceWorkbenchContext }
  | { kind: "context-request" }
  | { kind: "reveal"; reveal: FaceAssetReveal };

function postFaceWorkbenchMessage(message: FaceWorkbenchMessage): void {
  try {
    const channel = new BroadcastChannel(FACE_WORKBENCH_CHANNEL);
    channel.postMessage(message);
    channel.close();
  } catch {
    // Without BroadcastChannel the demo's two windows simply stay independent.
  }
}

function subscribeFaceWorkbenchMessages(
  callback: (message: FaceWorkbenchMessage) => void,
): UnlistenFn {
  try {
    const channel = new BroadcastChannel(FACE_WORKBENCH_CHANNEL);
    channel.addEventListener("message", (event) => callback(event.data as FaceWorkbenchMessage));
    return () => channel.close();
  } catch {
    return () => {};
  }
}

export async function openFaceWorkbench(): Promise<void> {
  if (!isTauri()) {
    window.open("?workbench=faces", "oxyviewer-face-workbench", "popup,width=1320,height=880");
    return;
  }
  await invoke("open_face_workbench_window");
}

export async function closeFaceWorkbench(): Promise<void> {
  if (!isTauri()) {
    window.close();
    return;
  }
  await invoke("close_face_workbench_window");
}

export async function isFaceWorkbenchOpen(): Promise<boolean> {
  if (!isTauri()) return false;
  return invoke<boolean>("is_face_workbench_window_open");
}

export async function onFaceWorkbenchVisibility(
  callback: (visible: boolean) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) {
    return subscribeFaceWorkbenchMessages((message) => {
      if (message.kind === "visibility") callback(message.visible);
    });
  }
  return listen<boolean>("face-workbench-visibility", (event) => callback(event.payload));
}

/** Main window → workbench: the browse scope and locale a run may use. */
export async function publishFaceWorkbenchContext(
  context: FaceWorkbenchContext,
): Promise<void> {
  if (!isTauri()) {
    postFaceWorkbenchMessage({ kind: "context", context });
    return;
  }
  await invoke("publish_face_workbench_context", { context });
}

/** Workbench → main window: "I just mounted, publish the current scope". */
export async function requestFaceWorkbenchContext(): Promise<void> {
  if (!isTauri()) {
    postFaceWorkbenchMessage({ kind: "context-request" });
    return;
  }
  await invoke("request_face_workbench_context");
}

export async function onFaceWorkbenchContext(
  callback: (context: FaceWorkbenchContext) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) {
    return subscribeFaceWorkbenchMessages((message) => {
      if (message.kind === "context") callback(message.context);
    });
  }
  return listen<FaceWorkbenchContext>("face-workbench-context", (event) => callback(event.payload));
}

export async function onFaceWorkbenchContextRequest(
  callback: () => void,
): Promise<UnlistenFn> {
  if (!isTauri()) {
    return subscribeFaceWorkbenchMessages((message) => {
      if (message.kind === "context-request") callback();
    });
  }
  return listen("face-workbench-context-request", () => callback());
}

/** Workbench → main window: show the asset one clicked face came from. */
export async function notifyFaceAssetReveal(reveal: FaceAssetReveal): Promise<void> {
  if (!isTauri()) {
    postFaceWorkbenchMessage({ kind: "reveal", reveal });
    return;
  }
  await invoke("notify_face_asset_reveal", { reveal });
}

export async function onFaceAssetReveal(
  callback: (reveal: FaceAssetReveal) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) {
    return subscribeFaceWorkbenchMessages((message) => {
      if (message.kind === "reveal") callback(message.reveal);
    });
  }
  return listen<FaceAssetReveal>("face-asset-reveal", (event) => callback(event.payload));
}

export async function getFaceSyncStatus(): Promise<import("@/types").FaceSyncStatus[]> {
  if (!isTauri()) return [];
  return invoke("get_face_sync_status");
}

export async function resolveFaceSyncConflict(path: string, useRemote: boolean): Promise<void> {
  if (!isTauri()) return;
  return invoke("resolve_face_sync_conflict", { path, useRemote });
}

export async function clearFaceAnalysisData(): Promise<void> {
  if (!isTauri()) {
    demoFacesInitialized = true;
    demoFaceReview = { items: [], total: 0, nextCursor: null };
    demoFaceClusters = [];
    demoFaceStats = { analyzedAssets: 0, facesDetected: 0, persons: demoFacePersons.length, pendingReviews: 0, unknownFaces: 0, clusters: 0 };
    return;
  }
  return invoke("clear_face_analysis_data");
}
export async function deletePeopleAnnotations(): Promise<void> {
  if (!isTauri()) { demoFacePersons.splice(0); await clearFaceAnalysisData(); return; }
  return invoke("delete_people_annotations");
}
