import { create } from "zustand";
import type { AssetSummary, MetadataPatch, MetadataProjection } from "../types";

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
  if (!projection) return asset;
  return {
    ...asset,
    rating: projection.rating,
    colorLabel: projection.colorLabel,
    pickLabel: projection.pickLabel,
  };
}

export function invalidateMetadataDirectory(directory: string) {
  useMetadataProjectionStore.getState().invalidateDirectory(directory);
}
