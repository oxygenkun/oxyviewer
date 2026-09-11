import { useCallback, useSyncExternalStore } from "react";
import type { AssetSummary, PreviewGeometry } from "../types";
import { validPreviewGeometry } from "./previewGeometry";

export const FOLDER_THUMBNAIL_MAX_EDGE = 512;

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
const captures = new Map<string, Promise<FolderThumbnail | undefined>>();
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
  captures.clear();
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
  captures.clear();
  listeners.get(path)?.forEach((listener) => listener());
  resetListeners.forEach((listener) => listener());
}

function scaledGeometry(
  geometry: PreviewGeometry | undefined,
  original: { width: number; height: number },
  resized: { width: number; height: number },
): PreviewGeometry | undefined {
  const valid = validPreviewGeometry(geometry, original);
  if (!valid) return undefined;
  const rect = valid.contentRect;
  const x = Math.min(resized.width - 1, Math.round(rect.x * resized.width / original.width));
  const y = Math.min(resized.height - 1, Math.round(rect.y * resized.height / original.height));
  return {
    displaySize: valid.displaySize,
    contentRect: {
      x, y,
      width: Math.max(1, Math.round((rect.x + rect.width) * resized.width / original.width) - x),
      height: Math.max(1, Math.round((rect.y + rect.height) * resized.height / original.height) - y),
    },
  };
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
export function captureFolderThumbnail(
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
  const pending = captures.get(key);
  if (pending) return pending;
  latestKeys.set(asset.path, key);
  const capture = async () => {
    const original = { width: image.naturalWidth, height: image.naturalHeight };
    if (!original.width || !original.height) return undefined;
    const scale = Math.min(1, FOLDER_THUMBNAIL_MAX_EDGE / Math.max(original.width, original.height));
    const width = Math.max(1, Math.round(original.width * scale));
    const height = Math.max(1, Math.round(original.height * scale));
    let url: string;
    let retained: HTMLImageElement;
    if (encoded && scale === 1) {
      // The preload owns this Blob URL. Transfer its already decoded image,
      // preserving the browser's color/orientation handling without re-encoding.
      url = image.src;
      retained = image;
    } else {
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("Folder thumbnail canvas is unavailable");
      context.drawImage(image, 0, 0, width, height);
      const blob = await new Promise<Blob>((resolve, reject) => {
        // Lossless: camera color/orientation has already been applied by WebView.
        canvas.toBlob((value) => value ? resolve(value) : reject(new Error("Thumbnail encoding failed")), "image/png");
      });
      // Drop the temporary backing store before decoding the retained small PNG.
      canvas.width = 0;
      canvas.height = 0;
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
      geometry: scaledGeometry(geometry, original, { width, height }),
    };
    const previous = thumbnails.get(asset.path);
    if (previous) release(previous);
    thumbnails.set(asset.path, thumbnail);
    listeners.get(asset.path)?.forEach((listener) => listener());
    return thumbnail;
  };
  const promise = capture().finally(() => {
    if (captures.get(key) === promise) captures.delete(key);
  });
  captures.set(key, promise);
  return promise;
}

export async function preloadFolderThumbnail(
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
  signal?.throwIfAborted();
  if (expectedGeneration !== generation) return;
  const ownedUrl = URL.createObjectURL(blob);
  let image: HTMLImageElement | undefined;
  try {
    image = await loadImage(ownedUrl, signal);
    signal?.throwIfAborted();
    await captureFolderThumbnail(asset, image, geometry, expectedGeneration, blob);
  } finally {
    if (!image || getFolderThumbnail(asset)?.image !== image) {
      if (image) image.src = "";
      URL.revokeObjectURL(ownedUrl);
    }
  }
}
