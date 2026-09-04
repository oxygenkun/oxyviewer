import { create } from "zustand";
import type { AssetSummary, MetadataProjection } from "../types";

interface MetadataProjectionState {
  records: Record<string, MetadataProjection>;
  accept: (projection: MetadataProjection) => void;
  invalidateDirectory: (directory: string) => void;
}

/** Read-only mirror of projections committed by the Rust coordinator. */
export const useMetadataProjectionStore = create<MetadataProjectionState>((set) => ({
  records: {},
  accept: (projection) => set((state) => {
    const current = state.records[projection.path];
    if (current && current.projectionRevision >= projection.projectionRevision) return state;
    return { records: { ...state.records, [projection.path]: projection } };
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

export function projectAssetMetadata(
  asset: AssetSummary,
  projection?: MetadataProjection,
): AssetSummary {
  if (!projection) return asset;
  return { ...asset, rating: projection.rating, colorLabel: projection.colorLabel };
}

export function invalidateMetadataDirectory(directory: string) {
  useMetadataProjectionStore.getState().invalidateDirectory(directory);
}
