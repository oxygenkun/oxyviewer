import { create } from "zustand";
import type { AssetSummary, MetadataPatch, MetadataProjection } from "../types";

const projectedAssets = new WeakMap<AssetSummary, AssetSummary>();
const pendingNotifications = new Map<string, MetadataProjection>();
let notificationFrame: number | undefined;
let notificationTimer: ReturnType<typeof setTimeout> | undefined;

export function queueMetadataProjection(projection: MetadataProjection): void {
  const pending = pendingNotifications.get(projection.path);
  if (!pending || projection.stateRevision > pending.stateRevision) pendingNotifications.set(projection.path, projection);
  if (notificationTimer !== undefined) return;
  notificationFrame = requestAnimationFrame(flushMetadataProjections);
  notificationTimer = setTimeout(flushMetadataProjections, 32);
}

export function flushMetadataProjections(): void {
  if (notificationFrame !== undefined) cancelAnimationFrame(notificationFrame);
  if (notificationTimer !== undefined) clearTimeout(notificationTimer);
  notificationFrame = undefined;
  notificationTimer = undefined;
  const projections = [...pendingNotifications.values()];
  pendingNotifications.clear();
  if (projections.length) acceptMetadataProjections(projections);
}

interface MetadataProjectionState {
  records: Record<string, MetadataProjection>;
  accept: (projection: MetadataProjection) => void;
  acceptMany: (projections: readonly MetadataProjection[]) => void;
  invalidateDirectory: (directory: string) => void;
}

/** Read-only mirror of projections committed by the Rust coordinator. */
export const useMetadataProjectionStore = create<MetadataProjectionState>((set) => ({
  records: {},
  accept: (projection) => set((state) => {
    const current = state.records[projection.path];
    if (current && current.stateRevision >= projection.stateRevision) return state;
    return { records: { ...state.records, [projection.path]: projection } };
  }),
  acceptMany: (projections) => set((state) => {
    let records = state.records;
    for (const projection of projections) {
      const current = records[projection.path];
      if (current && current.stateRevision >= projection.stateRevision) continue;
      if (records === state.records) records = { ...state.records };
      records[projection.path] = projection;
    }
    return records === state.records ? state : { records };
  }),
  invalidateDirectory: (directory) => set((state) => {
    const normalized = directory.replace(/[\\/]+$/, "").toLocaleLowerCase();
    return {
      records: Object.fromEntries(Object.entries(state.records).filter(([path]) => {
        const parent = path.replace(/[\\/][^\\/]+$/, "").toLocaleLowerCase();
        return parent !== normalized;
      })),
    };
  }),
}));

export function acceptMetadataProjection(projection: MetadataProjection) {
  useMetadataProjectionStore.getState().accept(projection);
}

export function acceptMetadataProjections(projections: readonly MetadataProjection[]) {
  useMetadataProjectionStore.getState().acceptMany(projections);
}

export function applyMetadataProjectionPatch(paths: string[], patch: MetadataPatch) {
  const selectedPaths = new Set(paths);
  useMetadataProjectionStore.setState((state) => {
    let changed = false;
    const records = { ...state.records };
    for (const [path, current] of Object.entries(records)) {
      if (!selectedPaths.has(path)) continue;
      records[path] = {
        ...current,
        ...(Object.hasOwn(patch, "rating") ? { rating: patch.rating ?? undefined } : {}),
        ...(Object.hasOwn(patch, "colorLabel")
          ? { colorLabel: patch.colorLabel ?? undefined }
          : {}),
        ...(Object.hasOwn(patch, "pickLabel")
          ? { pickLabel: patch.pickLabel ?? undefined }
          : {}),
      };
      changed = true;
    }
    return changed ? { records } : state;
  });
}

export function projectAssetMetadata(
  asset: AssetSummary,
  projection?: MetadataProjection,
): AssetSummary {
  if (!projection || (asset.rating === projection.rating && asset.colorLabel === projection.colorLabel && asset.pickLabel === projection.pickLabel)) return asset;
  const previous = projectedAssets.get(asset);
  if (previous && previous.rating === projection.rating && previous.colorLabel === projection.colorLabel && previous.pickLabel === projection.pickLabel) return previous;
  const projected = {
    ...asset,
    rating: projection.rating,
    colorLabel: projection.colorLabel,
    pickLabel: projection.pickLabel,
  };
  projectedAssets.set(asset, projected);
  return projected;
}

/** Subscribe only to one asset's visible fields; revision-only changes are inert. */
export function useAssetMetadata(asset: AssetSummary): AssetSummary {
  return useMetadataProjectionStore((state) => projectAssetMetadata(asset, state.records[asset.path]));
}

export function invalidateMetadataDirectory(directory: string) {
  const normalized = directory.replace(/[\\/]+$/, "").toLocaleLowerCase();
  for (const path of pendingNotifications.keys()) {
    if (path.replace(/[\\/][^\\/]+$/, "").toLocaleLowerCase() === normalized) pendingNotifications.delete(path);
  }
  useMetadataProjectionStore.getState().invalidateDirectory(directory);
}
