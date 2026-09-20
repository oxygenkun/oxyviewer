import { releaseUnretainedMediaResource } from "@/lib/cache/mediaResourceLease";
import { create } from "zustand";
import type { ImageProjection, PreviewResult, RenderLevel } from "@/types";
import {
  clearBrowserImageResources,
  discardBrowserImageResource,
} from "@/lib/cache/browserImageCache";
import { mediaProtocolUrl } from "@/lib/media/mediaProtocolUrl";
import { sharedThumbnailRequests } from "@/lib/preview/sharedThumbnailRequests";
import { discardFolderThumbnail } from "@/lib/cache/folderThumbnailCache";

export interface ImageProjectionMirror extends Omit<ImageProjection, "result"> {
  result?: PreviewResult;
}

interface ImageProjectionState {
  records: Record<string, ImageProjectionMirror>;
  accept: (projection: ImageProjection) => void;
  invalidateDirectory: (directory: string) => void;
  clear: () => void;
}

export const imageProjectionKey = (path: string, level: RenderLevel) => `${path}\0${level}`;
const retiredSources = new Map<string, string>();

/** Read-only frontend mirror of image artifacts accepted by Rust. */
function projectionResultUrl(result: Omit<PreviewResult, "url">): string {
  if (result.resource) return mediaProtocolUrl(result.resource.url);
  const nativeWindow = window as Window & { __TAURI_INTERNALS__?: unknown };
  return nativeWindow.__TAURI_INTERNALS__ ? "" : result.path;
}

export const useImageProjectionStore = create<ImageProjectionState>((set) => ({
  records: {},
  accept: (projection) => set((state) => {
    const key = imageProjectionKey(projection.path, projection.level);
    const current = state.records[key];
    if (current && current.stateRevision >= projection.stateRevision) return state;
    const result = projection.result
      ? {
          ...projection.result,
          url: projectionResultUrl(projection.result),
        }
      : current?.sourceRevision === projection.sourceRevision
        ? current.result
        : undefined;
    return {
      records: {
        ...state.records,
        [key]: { ...projection, result },
      },
    };
  }),
  invalidateDirectory: (directory) => set((state) => {
    const normalized = directory.replace(/[\\/]+$/, "").toLocaleLowerCase();
    return {
      records: Object.fromEntries(Object.entries(state.records).filter(([, projection]) => {
        const parent = projection.path.replace(/[\\/][^\\/]+$/, "").toLocaleLowerCase();
        return parent !== normalized;
      })),
    };
  }),
  clear: () => set({ records: {} }),
}));

export function acceptImageProjection(projection: ImageProjection) {
  const retiredKey = imageProjectionKey(projection.path, projection.level);
  if (retiredSources.get(retiredKey) === projection.sourceRevision) {
    const id = projection.result?.resource?.resourceId;
    if (id) releaseUnretainedMediaResource(id);
    return false;
  }
  const current = useImageProjectionStore.getState().records[
    imageProjectionKey(projection.path, projection.level)
  ];
  if (current && current.stateRevision >= projection.stateRevision) {
    const id = projection.result?.resource?.resourceId;
    if (id && id !== current.result?.resource?.resourceId) releaseUnretainedMediaResource(id);
    return id !== undefined && id === current.result?.resource?.resourceId;
  }
  if (current && current.stateRevision < projection.stateRevision) {
    const id = current.result?.resource?.resourceId;
    if (id && (projection.result || current.sourceRevision !== projection.sourceRevision)
      && id !== projection.result?.resource?.resourceId) releaseUnretainedMediaResource(id);
    // Registered resources have immutable URLs. Persistence/status updates for
    // the same resource must not discard already decoded pixels.
    const sourceChanged = current.sourceRevision !== projection.sourceRevision;
    const nextUrl = projection.result ? projectionResultUrl(projection.result) : undefined;
    const mutablePathReplaced = Boolean(projection.result && !projection.result.resource);
    if (sourceChanged || mutablePathReplaced) {
      discardFolderThumbnail(projection.path);
      discardBrowserImageResource(current.result?.url);
      discardBrowserImageResource(nextUrl);
    }
  }
  useImageProjectionStore.getState().accept(projection);
  return true;
}

export function invalidateImageDirectory(directory: string) {
  sharedThumbnailRequests.invalidate(directory);
  clearBrowserImageResources();
  useImageProjectionStore.getState().invalidateDirectory(directory);
}

/** Releases only one removed asset while preserving the current folder's decoded thumbnails. */
export function invalidateImageAsset(path: string) {
  sharedThumbnailRequests.invalidatePath(path);
  const records = { ...useImageProjectionStore.getState().records };
  let changed = false;
  for (const [key, projection] of Object.entries(records)) {
    if (projection.path !== path) continue;
    const id = projection.result?.resource?.resourceId;
    if (id) releaseUnretainedMediaResource(id);
    discardBrowserImageResource(projection.result?.url);
    retiredSources.delete(key);
    delete records[key];
    changed = true;
  }
  discardFolderThumbnail(path);
  if (changed) useImageProjectionStore.setState({ records });
}

export function clearImageProjections() {
  retiredSources.clear();
  sharedThumbnailRequests.invalidate();
  clearBrowserImageResources();
  useImageProjectionStore.getState().clear();
}

/** Regeneration retains other levels and the currently displayed resource lease. */
export function invalidateImageProjection(path: string, level: RenderLevel) {
  const records = { ...useImageProjectionStore.getState().records };
  for (const [key, projection] of Object.entries(records)) {
    if (projection.level !== level || projection.path !== path) continue;
    const id = projection.result?.resource?.resourceId;
    if (id) releaseUnretainedMediaResource(id);
    retiredSources.delete(key);
    retiredSources.set(key, projection.sourceRevision);
    if (retiredSources.size > 256) retiredSources.delete(retiredSources.keys().next().value!);
    delete records[key];
  }
  useImageProjectionStore.setState({ records });
}
