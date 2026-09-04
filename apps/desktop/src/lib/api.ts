import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AssetDetails,
  AssetDetailsResult,
  AssetKind,
  AssetQuery,
  AssetSummary,
  CacheSettings,
  DirectorySearchMatch,
  DirectorySummary,
  DirectoryTreeNode,
  DirectoryTreeSnapshot,
  FolderSession,
  HeifCapabilities,
  HeifDecodeSession,
  HeifDiagnostics,
  ImageProjection,
  LibraryIndexUpdate,
  ExiftoolStatus,
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
  CustomTag,
  DebugQueueSnapshot,
  TagDeleteImpact,
  TagSyncStatus,
} from "../types";
import { preloadBrowserImage } from "./browserImageCache";
import { browserPreloadQueue, priorityWeight } from "./previewQueue";
import { acceptImageProjection } from "./imageProjection";
import { acceptMetadataProjection } from "./metadataProjection";
import { perfMark } from "./perfProbe";
import { beginPreviewDebug } from "./previewDebug";

function cancelGeneratedPreviewRequest(
  asset: AssetSummary,
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
    const session = {
      id: "demo-session",
      rootPath: path,
      displayName: "Field Notes",
      openedAtMs: Date.now(),
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
): Promise<Page<AssetSummary>> {
  if (!isTauri()) {
    const needle = query.search?.toLowerCase();
    const filtered = [...demoAssets]
      .filter((asset) => needle || asset.path.slice(0, asset.path.lastIndexOf("/")) === directory)
      .filter((asset) => !query.kind || asset.kind === query.kind)
      .filter((asset) => !query.minimumRating || (asset.rating ?? 0) >= query.minimumRating)
      .filter((asset) => !query.colorLabels?.length ||
        query.colorLabels.some((label) => asset.colorLabel?.toLowerCase() === label.toLowerCase()))
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
  const page = await invoke<Page<AssetSummary>>("list_assets", {
    sessionId,
    directory,
    query,
    cursor,
  });
  if (!cursor) {
    perfMark("assets:first-page-returned", { directory, total: page.total });
  }
  return page;
}

export async function onLibraryIndexUpdated(
  callback: (update: LibraryIndexUpdate) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<LibraryIndexUpdate>("library-index-updated", (event) => callback(event.payload));
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
    const node = findDemoTreeNode(snapshot.root, directory);
    if (node && (node.expanded || node.children !== null)) {
      node.children = demoTreeChildren(directory, node.children ?? []);
      node.entry.hasChildren = node.children.length > 0;
      if (node.children.length === 0) node.expanded = false;
    }
    snapshot.revision = ++demoTreeRevision;
    return cloneDemoTree(snapshot);
  }
  return invoke<DirectoryTreeSnapshot>("refresh_directory", { sessionId, directory });
}

export async function trashPaths(paths: string[]): Promise<void> {
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
    operation: { type: "trash", paths },
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
      projectionRevision: Date.now() + index,
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
    for (const asset of demoAssets.filter((asset) => paths.includes(asset.path))) {
      if ("rating" in patch) asset.rating = patch.rating ?? undefined;
      if ("colorLabel" in patch) asset.colorLabel = patch.colorLabel ?? undefined;
      if ("pickLabel" in patch) asset.pickLabel = patch.pickLabel ?? undefined;
      asset.hasSidecar = true;
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
  return invoke<CustomTag[]>("list_custom_tags");
}

export async function getAssetTagAssignments(paths: string[]): Promise<AssetTagAssignment[]> {
  if (!isTauri()) {
    return demoTags.map((tag) => ({
      tag: { ...tag },
      assignedCount: paths.filter((path) => demoAssetTags.get(path)?.has(tag.id)).length,
      assetCount: paths.length,
    }));
  }
  return invoke<AssetTagAssignment[]>("get_asset_tag_assignments", { paths });
}

export async function createCustomTag(parentId: number | undefined, name: string): Promise<CustomTag> {
  if (!isTauri()) {
    const siblings = demoTags.filter((tag) => tag.parentId === parentId);
    const tag: CustomTag = { id: demoTagId++, parentId, name: name.trim(), path: "", sortOrder: siblings.length };
    demoTags.push(tag);
    refreshDemoTagPaths();
    return { ...demoTags.find((item) => item.id === tag.id)! };
  }
  return invoke<CustomTag>("create_custom_tag", { parentId: parentId ?? null, name });
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
  return invoke<CustomTag>("update_custom_tag", { id, parentId: parentId ?? null, name });
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
      if (assigned) ids.add(tagId); else ids.delete(tagId);
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
    return { capturedAtUnixMs: Date.now(), queues: [] };
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
  if (!isTauri() || asset.kind === "raw" || asset.kind === "heif" || asset.kind === "tiff") {
    return undefined;
  }
  return convertFileSrc(asset.path);
}

export async function generatedPreview(
  asset: AssetSummary,
  level: RenderLevel,
  signal?: AbortSignal,
  priority: PreviewPriority = "visible",
  queueOrder = 0,
): Promise<PreviewResult | undefined> {
  if (!isTauri()) return undefined;
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
    queueOrder,
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
  try {
    const projection = await (signal ? Promise.race([backendRequest, aborted]) : backendRequest);
    if (signal?.aborted) throw signal.reason ?? new DOMException("Aborted", "AbortError");
    acceptImageProjection(projection);
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
    return { ...result, url: convertFileSrc(result.path) };
  } catch (error) {
    if (signal?.aborted) debug?.cancel();
    else debug?.fail(error);
    throw error;
  } finally {
    signal?.removeEventListener("abort", stopWaiting);
  }
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
 * Warms both the backend preview cache and the webview image cache. Filtered
 * results use the dedicated lowest queue priority so ordinary overscan can
 * always jump ahead of this work.
 */
export async function preloadAssetThumbnail(
  asset: AssetSummary,
  signal?: AbortSignal,
): Promise<void> {
  if (!isTauri()) return;
  const directSource = previewUrl(asset);
  if (directSource) {
    await browserPreloadQueue.enqueue(
      priorityWeight("preload"),
      signal,
      () => preloadBrowserImage(directSource, signal),
    );
    return;
  }
  const result = await generatedPreview(asset, "thumbnail", signal, "preload");
  if (result) await preloadBrowserImage(result.url, signal);
}

export async function startHeifDecode(
  path: string,
  generation: number,
  hardwareAcceleration: boolean,
  displaySharpening: boolean,
): Promise<HeifDecodeSession> {
  perfMark("heif:decode-requested", { path });
  const session = await invoke<HeifDecodeSession>("start_heif_decode", {
    path,
    generation,
    hardwareAcceleration,
    displaySharpening,
  });
  perfMark("heif:decode-session", { path, sessionId: session.id });
  return session;
}

export async function getCachedHeifFull(
  path: string,
): Promise<PreviewResult | undefined> {
  if (!isTauri()) return undefined;
  const result = await invoke<Omit<PreviewResult, "url"> | null>("get_cached_heif_full", {
    path,
  });
  return result ? { ...result, url: convertFileSrc(result.path) } : undefined;
}

export async function cancelHeifDecode(sessionId: string): Promise<boolean> {
  return invoke<boolean>("cancel_heif_decode", { sessionId });
}

export async function getHeifCapabilities(): Promise<HeifCapabilities[]> {
  if (!isTauri()) return [];
  return invoke<HeifCapabilities[]>("get_heif_capabilities");
}

export async function getHeifDiagnostics(): Promise<HeifDiagnostics | undefined> {
  if (!isTauri()) return undefined;
  return invoke<HeifDiagnostics | undefined>("get_heif_diagnostics");
}

export function heifTileUrl(url: string): string {
  return navigator.userAgent.includes("Windows")
    ? url.replace("oxy-media://localhost", "http://oxy-media.localhost")
    : url;
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
