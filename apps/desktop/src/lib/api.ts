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
  PreviewResult,
  RenderLevel,
} from "../types";
import { preloadBrowserImage } from "./browserImageCache";
import { browserPreloadQueue, priorityWeight } from "./previewQueue";
import { acceptImageProjection } from "./imageProjection";
import { acceptMetadataProjection } from "./metadataProjection";
import { perfMark } from "./perfProbe";
import { beginPreviewDebug } from "./previewDebug";

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
  { path: `${demoRoot}/Portraits`, name: "Portraits", hasChildren: false },
  { path: `${demoRoot}/Trips`, name: "Trips", hasChildren: true },
  { path: `${demoRoot}/Trips/Coast`, name: "Coast", hasChildren: false },
];

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
}));

export const isTauri = () => "__TAURI_INTERNALS__" in window;

export async function chooseFolder(): Promise<string | null> {
  if (!isTauri()) return "/demo/Field Notes";
  const selection = await open({ directory: true, multiple: false });
  return typeof selection === "string" ? selection : null;
}

export async function openFolder(path: string): Promise<FolderSession> {
  if (!isTauri()) {
    return {
      id: "demo-session",
      rootPath: path,
      displayName: "Field Notes",
      openedAtMs: Date.now(),
    };
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

export async function listDirectories(
  sessionId: string,
  directory: string,
): Promise<DirectorySummary[]> {
  if (!isTauri()) {
    return demoDirectories.filter((entry) => {
      const parent = entry.path.slice(0, entry.path.lastIndexOf("/"));
      return parent === directory;
    });
  }
  return invoke<DirectorySummary[]>("list_directories", { sessionId, directory });
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

export async function refreshDirectory(sessionId: string, directory: string): Promise<void> {
  if (!isTauri()) return;
  await invoke("refresh_directory", { sessionId, directory });
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
        creator: "OxyViewer Demo",
        copyright: "Personal archive",
        keywords: ["field-notes", asset.kind],
      },
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
      asset.hasSidecar = true;
    }
    return "demo-metadata-job";
  }
  return invoke<string>("patch_metadata", { paths, patch });
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
  const debug = __OXY_DEBUG__
    ? beginPreviewDebug({
        assetName: asset.name,
        stage: level,
        priority,
      })
    : undefined;
  if (signal?.aborted) throw signal.reason ?? new DOMException("Aborted", "AbortError");
  debug?.start();
  perfMark("preview:queued", { assetName: asset.name, level, priority });
  try {
    const projection = await invoke<ImageProjection>("get_preview", {
      path: asset.path,
      level,
      priority,
      queueOrder,
    });
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
  }
}

export function raiseGeneratedPreviewPriority(
  asset: AssetSummary,
  level: RenderLevel,
  priority: PreviewPriority,
  queueOrder = 0,
): boolean {
  if (!isTauri()) return false;
  void invoke("get_preview", {
    path: asset.path,
    level,
    priority,
    queueOrder,
  }).catch(() => undefined);
  return true;
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
