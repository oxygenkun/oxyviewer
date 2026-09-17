import { useCallback, useSyncExternalStore } from "react";
import type { AssetSummary, PreviewGeometry } from "@/types";
import { validPreviewGeometry } from "@/lib/preview/previewGeometry";
import { perfMark } from "@/lib/diagnostics/perfProbe";

export const FOLDER_THUMBNAIL_MAX_EDGE = 512;
const FOLDER_THUMBNAIL_MAX_BYTES = 2 * 1024 * 1024;

export interface FolderThumbnail {
  assetKey: string;
  url: string;
  width: number;
  height: number;
  geometry?: PreviewGeometry;
  // Own browser pixels and a Blob URL, not an expiring native resource lease.
  image: HTMLImageElement;
}

const thumbnails = new Map<string, FolderThumbnail>();
const retentions = new Map<string, Promise<FolderThumbnail | undefined>>();
interface PendingLoad { promise: Promise<void>; controller: AbortController; consumers: number }
const loads = new Map<string, PendingLoad>();
const latestKeys = new Map<string, string>();
const listeners = new Map<string, Set<() => void>>();
const resetListeners = new Set<() => void>();
let generation = 0;

export const folderThumbnailKey = (asset: AssetSummary) =>
  `${asset.path}\0${asset.modifiedAtMs}\0${asset.sizeBytes}`;

export function getFolderThumbnail(asset: AssetSummary): FolderThumbnail | undefined {
  const thumbnail = thumbnails.get(asset.path);
  return thumbnail?.assetKey === folderThumbnailKey(asset) ? thumbnail : undefined;
}

export function useFolderThumbnail(asset: AssetSummary): FolderThumbnail | undefined {
  const subscribe = useCallback((listener: () => void) => {
    let subscribers = listeners.get(asset.path);
    if (!subscribers) listeners.set(asset.path, subscribers = new Set());
    subscribers.add(listener);
    return () => {
      subscribers.delete(listener);
      if (!subscribers.size) listeners.delete(asset.path);
    };
  }, [asset.path]);
  return useSyncExternalStore(subscribe, () => getFolderThumbnail(asset));
}

export const getFolderThumbnailGeneration = () => generation;
export function getFolderThumbnailStats() {
  let decodedBytes = 0;
  for (const thumbnail of thumbnails.values()) decodedBytes += thumbnail.width * thumbnail.height * 4;
  return { count: thumbnails.size, decodedBytes, generation };
}
const subscribeResets = (listener: () => void) => {
  resetListeners.add(listener);
  return () => { resetListeners.delete(listener); };
};
export function useFolderThumbnailGeneration(): number {
  return useSyncExternalStore(subscribeResets, getFolderThumbnailGeneration);
}

function release(thumbnail: FolderThumbnail): void {
  thumbnail.image.src = "";
  URL.revokeObjectURL(thumbnail.url);
}

/** Directory/source invalidation also fences image loads still in flight. */
export function clearFolderThumbnails(): void {
  generation += 1;
  const paths = [...thumbnails.keys()];
  for (const thumbnail of thumbnails.values()) release(thumbnail);
  thumbnails.clear();
  retentions.clear();
  for (const load of loads.values()) load.controller.abort();
  loads.clear();
  latestKeys.clear();
  for (const path of paths) listeners.get(path)?.forEach((listener) => listener());
  resetListeners.forEach((listener) => listener());
}

export function discardFolderThumbnail(path: string): void {
  generation += 1;
  const thumbnail = thumbnails.get(path);
  if (thumbnail) release(thumbnail);
  thumbnails.delete(path);
  latestKeys.delete(path);
  retentions.clear();
  for (const load of loads.values()) load.controller.abort();
  loads.clear();
  listeners.get(path)?.forEach((listener) => listener());
  resetListeners.forEach((listener) => listener());
}

function loadImage(url: string, signal?: AbortSignal): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) { reject(signal.reason); return; }
    const image = new Image();
    // Native media responses permit CORS; snapshots must remain origin-clean.
    image.crossOrigin = "anonymous";
    const cleanup = () => {
      image.onload = null;
      image.onerror = null;
      signal?.removeEventListener("abort", abort);
    };
    const abort = () => {
      cleanup();
      image.src = "";
      reject(signal?.reason ?? new DOMException("Aborted", "AbortError"));
    };
    image.onload = () => {
      void image.decode().catch(() => undefined).then(() => {
        if (signal?.aborted) return;
        cleanup();
        resolve(image);
      });
    };
    image.onerror = () => {
      cleanup();
      image.src = "";
      reject(new Error("Failed to load folder thumbnail"));
    };
    signal?.addEventListener("abort", abort, { once: true });
    image.src = url;
  });
}

/** Retain all current-folder thumbnails; full-size browser images use their own LRU. */
export function retainFolderThumbnail(
  asset: AssetSummary,
  image: HTMLImageElement,
  geometry?: PreviewGeometry,
  expectedGeneration = generation,
  encoded?: Blob,
): Promise<FolderThumbnail | undefined> {
  if (expectedGeneration !== generation) return Promise.resolve(undefined);
  const cached = getFolderThumbnail(asset);
  if (cached) return Promise.resolve(cached);
  const key = folderThumbnailKey(asset);
  const pending = retentions.get(key);
  if (pending) return pending;
  latestKeys.set(asset.path, key);
  const retain = async () => {
    const original = { width: image.naturalWidth, height: image.naturalHeight };
    if (!original.width || !original.height) return undefined;
    if (Math.max(original.width, original.height) > FOLDER_THUMBNAIL_MAX_EDGE) {
      throw new Error("Native thumbnail exceeds the 512-pixel delivery bound");
    }
    const { width, height } = original;
    let url: string;
    let retained: HTMLImageElement;
    if (encoded) {
      if (encoded.size > FOLDER_THUMBNAIL_MAX_BYTES) throw new Error("Native thumbnail exceeds the encoded byte limit");
      // The preload owns this Blob URL. Transfer its already decoded image,
      // preserving the browser's color/orientation handling without re-encoding.
      url = image.src;
      retained = image;
    } else {
      const response = await fetch(image.currentSrc || image.src);
      if (!response.ok) throw new Error(`Folder thumbnail fetch failed: ${response.status}`);
      const blob = await response.blob();
      if (blob.size > FOLDER_THUMBNAIL_MAX_BYTES) throw new Error("Native thumbnail exceeds the encoded byte limit");
      if (expectedGeneration !== generation) return undefined;
      url = URL.createObjectURL(blob);
      try { retained = await loadImage(url); }
      catch (error) { URL.revokeObjectURL(url); throw error; }
    }
    if (expectedGeneration !== generation || latestKeys.get(asset.path) !== key) {
      retained.src = "";
      URL.revokeObjectURL(url);
      return undefined;
    }
    const thumbnail: FolderThumbnail = {
      assetKey: key, url, width, height, image: retained,
      geometry: validPreviewGeometry(geometry, original),
    };
    const previous = thumbnails.get(asset.path);
    if (previous) release(previous);
    thumbnails.set(asset.path, thumbnail);
    perfMark("thumbnail:retained", { assetName: asset.name, width, height, count: thumbnails.size });
    listeners.get(asset.path)?.forEach((listener) => listener());
    return thumbnail;
  };
  const promise = retain().finally(() => {
    if (retentions.get(key) === promise) retentions.delete(key);
  });
  retentions.set(key, promise);
  return promise;
}

async function loadFolderThumbnail(
  asset: AssetSummary,
  url: string,
  geometry?: PreviewGeometry,
  signal?: AbortSignal,
  expectedGeneration = generation,
): Promise<void> {
  signal?.throwIfAborted();
  if (expectedGeneration !== generation || getFolderThumbnail(asset)) return;
  const response = await fetch(url, { signal });
  if (!response.ok) throw new Error(`Folder thumbnail fetch failed: ${response.status}`);
  const blob = await response.blob();
  if (blob.size > FOLDER_THUMBNAIL_MAX_BYTES) throw new Error("Native thumbnail exceeds the encoded byte limit");
  signal?.throwIfAborted();
  if (expectedGeneration !== generation) return;
  const ownedUrl = URL.createObjectURL(blob);
  let image: HTMLImageElement | undefined;
  try {
    image = await loadImage(ownedUrl, signal);
    signal?.throwIfAborted();
    await retainFolderThumbnail(asset, image, geometry, expectedGeneration, blob);
  } finally {
    if (!image || getFolderThumbnail(asset)?.image !== image) {
      if (image) image.src = "";
      URL.revokeObjectURL(ownedUrl);
    }
  }
}

/** Visible mounts and background preloads share fetch/decode work by source.
 * A cancelled consumer releases only its subscription; the last one cancels I/O.
 */
export async function preloadFolderThumbnail(
  asset: AssetSummary, url: string, geometry?: PreviewGeometry, signal?: AbortSignal,
  expectedGeneration = generation,
): Promise<void> {
  signal?.throwIfAborted();
  if (expectedGeneration !== generation || getFolderThumbnail(asset)) return;
  const key = folderThumbnailKey(asset);
  let load = loads.get(key);
  if (!load || load.controller.signal.aborted) {
    const controller = new AbortController();
    const created: PendingLoad = { controller, consumers: 0, promise: Promise.resolve() };
    created.promise = loadFolderThumbnail(asset, url, geometry, controller.signal, expectedGeneration)
      .finally(() => { if (loads.get(key) === created) loads.delete(key); });
    load = created;
    loads.set(key, created);
  }
  const shared = load;
  shared.consumers += 1;
  let abort: (() => void) | undefined;
  try {
    await new Promise<void>((resolve, reject) => {
      abort = () => reject(signal?.reason ?? new DOMException("Aborted", "AbortError"));
      signal?.addEventListener("abort", abort, { once: true });
      shared.promise.then(resolve, reject);
      if (signal?.aborted) abort();
    });
  } finally {
    if (abort) signal?.removeEventListener("abort", abort);
    shared.consumers -= 1;
    if (!shared.consumers && loads.get(key) === shared) shared.controller.abort();
  }
}
