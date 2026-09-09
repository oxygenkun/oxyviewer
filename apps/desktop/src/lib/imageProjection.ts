import { releaseUnretainedMediaResource } from "./mediaResourceLease";
import { create } from "zustand";
import type { ImageProjection, PreviewResult, RenderLevel } from "../types";
import {
  clearBrowserImageResources,
  discardBrowserImageResource,
} from "./browserImageCache";
import { mediaProtocolUrl } from "./mediaProtocolUrl";

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
    // A projection update may overwrite the same cache path. Drop both URL
    // identities so the WebView cannot repaint a retained stale decode.
    discardBrowserImageResource(current.result?.url);
    discardBrowserImageResource(
      projection.result ? projectionResultUrl(projection.result) : undefined,
    );
  }
  useImageProjectionStore.getState().accept(projection);
  return true;
}

export function invalidateImageDirectory(directory: string) {
  clearBrowserImageResources();
  useImageProjectionStore.getState().invalidateDirectory(directory);
}

export function clearImageProjections() {
  clearBrowserImageResources();
  useImageProjectionStore.getState().clear();
}
