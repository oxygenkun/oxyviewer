import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AssetDetails,
  AssetKind,
  AssetQuery,
  AssetSummary,
  DirectorySummary,
  FolderSession,
  HeifCapabilities,
  HeifDecodeSession,
  HeifDiagnostics,
  MetadataPatch,
  Page,
  PerfScenario,
  PreviewPriority,
  PreviewResult,
  RenderLevel,
} from "../types";
import { previewQueue, priorityWeight } from "./previewQueue";
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
      .filter((asset) => asset.path.slice(0, asset.path.lastIndexOf("/")) === directory)
      .filter((asset) => !query.kind || asset.kind === query.kind)
      .filter((asset) => !query.minimumRating || (asset.rating ?? 0) >= query.minimumRating)
      .filter((asset) => !query.colorLabel || asset.colorLabel === query.colorLabel)
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

export async function refreshDirectory(sessionId: string, directory: string): Promise<void> {
  if (!isTauri()) return;
  await invoke("refresh_directory", { sessionId, directory });
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
      sidecarPath: asset.hasSidecar ? asset.path.replace(/\.[^.]+$/, ".xmp") : undefined,
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
  return invoke<AssetDetails>("get_asset_details", { path: asset.path });
}

export async function patchMetadata(paths: string[], patch: MetadataPatch): Promise<string> {
  if (!isTauri()) {
    for (const asset of demoAssets.filter((asset) => paths.includes(asset.path))) {
      if ("rating" in patch) asset.rating = patch.rating ?? undefined;
      if ("colorLabel" in patch) asset.colorLabel = patch.colorLabel ?? undefined;
    }
    return "demo-metadata-job";
  }
  return invoke<string>("patch_metadata", { paths, patch });
}

const demoRoots = new Set<string>();

export async function addLibraryRoot(path: string): Promise<string[]> {
  if (!isTauri()) {
    demoRoots.add(path);
    return [...demoRoots];
  }
  return invoke<string[]>("add_library_root", { path });
}

export async function removeLibraryRoot(path: string): Promise<string[]> {
  if (!isTauri()) {
    demoRoots.delete(path);
    return [...demoRoots];
  }
  return invoke<string[]>("remove_library_root", { path });
}

export async function listLibraryRoots(): Promise<string[]> {
  if (!isTauri()) return [...demoRoots];
  return invoke<string[]>("list_library_roots");
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
): Promise<PreviewResult | undefined> {
  if (!isTauri()) return undefined;
  const debug = __OXY_DEBUG__
    ? beginPreviewDebug({
        assetName: asset.name,
        stage: level,
        priority,
      })
    : undefined;
  let submittedPriority = priority;
  const request = (effectiveWeight: number) => {
    const effectivePriority = previewPriorityForWeight(effectiveWeight);
    submittedPriority = effectivePriority;
    debug?.updatePriority(effectivePriority);
    debug?.start();
    perfMark("preview:queued", { assetName: asset.name, level, priority: effectivePriority });
    return invoke<Omit<PreviewResult, "url">>("get_preview", {
      path: asset.path,
      level,
      priority: effectivePriority,
    });
  };
  try {
    const result = await previewQueue.enqueue(
      priorityWeight(priority),
      signal,
      request,
      generatedPreviewTaskKey(asset, level),
    );
    perfMark("preview:result", {
      assetName: asset.name,
      level,
      priority: submittedPriority,
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

function generatedPreviewTaskKey(asset: AssetSummary, level: RenderLevel): string {
  return `${asset.id}:${asset.modifiedAtMs}:${level}`;
}

function previewPriorityForWeight(weight: number): PreviewPriority {
  if (weight >= priorityWeight("loupe")) return "loupe";
  if (weight >= priorityWeight("visible")) return "visible";
  if (weight >= priorityWeight("nearby")) return "nearby";
  return "preload";
}

export function raiseGeneratedPreviewPriority(
  asset: AssetSummary,
  level: RenderLevel,
  priority: PreviewPriority,
): boolean {
  return previewQueue.raisePriority(
    generatedPreviewTaskKey(asset, level),
    priorityWeight(priority),
  );
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
    await previewQueue.enqueue(
      priorityWeight("preload"),
      signal,
      () => preloadBrowserImage(directSource, signal),
    );
    return;
  }
  const result = await generatedPreview(asset, "thumbnail", signal, "preload");
  if (result) await preloadBrowserImage(result.url, signal);
}

function preloadBrowserImage(url: string, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason ?? new DOMException("Aborted", "AbortError"));
      return;
    }
    const image = new Image();
    const cleanup = () => {
      image.onload = null;
      image.onerror = null;
      signal?.removeEventListener("abort", abort);
    };
    const abort = () => {
      image.src = "";
      cleanup();
      reject(signal?.reason ?? new DOMException("Aborted", "AbortError"));
    };
    image.onload = () => {
      cleanup();
      resolve();
    };
    image.onerror = () => {
      cleanup();
      reject(new Error(`failed to preload ${url}`));
    };
    signal?.addEventListener("abort", abort, { once: true });
    image.src = url;
  });
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
