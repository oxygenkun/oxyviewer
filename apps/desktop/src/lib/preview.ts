export const THUMBNAIL_PREVIEW_SIZE = 512;
export const LOUPE_PREVIEW_SIZE = 4_096;

export function previewStages(large: boolean) {
  return large
    ? [THUMBNAIL_PREVIEW_SIZE, LOUPE_PREVIEW_SIZE] as const
    : [THUMBNAIL_PREVIEW_SIZE] as const;
}
