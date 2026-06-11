export type RawPreviewStatus =
  | { state: "loadingPreview" }
  | { state: "developingFull" }
  | { state: "fullReady"; width: number; height: number }
  | { state: "fullFailed" };

interface RawPreviewStateInput {
  assetId: string;
  loaded?: { assetId: string; mode: "preview" | "full" };
  fullError: boolean;
  fullSize?: { width: number; height: number };
}

export function rawPreviewStatus({
  assetId,
  loaded,
  fullError,
  fullSize,
}: RawPreviewStateInput): RawPreviewStatus {
  if (loaded?.assetId === assetId && loaded.mode === "full") {
    return {
      state: "fullReady",
      width: fullSize?.width ?? 0,
      height: fullSize?.height ?? 0,
    };
  }
  if (fullError) return { state: "fullFailed" };
  if (loaded?.assetId === assetId) return { state: "developingFull" };
  return { state: "loadingPreview" };
}
