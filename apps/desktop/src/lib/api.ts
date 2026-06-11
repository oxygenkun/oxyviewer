import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AssetDetails,
  AssetKind,
  AssetQuery,
  AssetSummary,
  DirectorySummary,
  FolderSession,
  Page,
  PreviewMode,
  PreviewResult,
} from "../types";

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
  return invoke<FolderSession>("open_folder", { path });
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
  return invoke<Page<AssetSummary>>("list_assets", {
    sessionId,
    directory,
    query,
    cursor,
  });
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
        rating: Number(asset.id.at(-1) ?? 0) % 6,
        colorLabel: asset.kind === "raw" ? "Amber" : undefined,
        creator: "OxyViewer Demo",
        copyright: "Personal archive",
        keywords: ["field-notes", asset.kind],
      },
      sidecarPath: asset.hasSidecar ? asset.path.replace(/\.[^.]+$/, ".xmp") : undefined,
    };
  }
  return invoke<AssetDetails>("get_asset_details", { path: asset.path });
}

export async function addLibraryRoot(path: string): Promise<string[]> {
  if (!isTauri()) return [path];
  return invoke<string[]>("add_library_root", { path });
}

export async function listLibraryRoots(): Promise<string[]> {
  if (!isTauri()) return [];
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
  mode: PreviewMode,
  maxSize?: number,
): Promise<PreviewResult | undefined> {
  if (!isTauri()) return undefined;
  const result = await invoke<Omit<PreviewResult, "url">>("get_preview", {
    path: asset.path,
    mode,
    maxSize,
  });
  return { ...result, url: convertFileSrc(result.path) };
}
