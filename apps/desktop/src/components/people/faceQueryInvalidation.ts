import type { QueryClient } from "@tanstack/react-query";

const keys = {
  analysis: ["face-capability", "face-clusters", "face-review", "face-photo-preview", "face-calibration", "asset-face-reviews"],
  people: ["custom-tags", "asset-tag-assignments", "face-capability", "face-persons", "face-clusters", "face-review", "face-photo-preview", "face-undo", "face-calibration", "asset-face-reviews"],
  incremental: ["face-capability", "face-review"],
} as const;

export type FaceInvalidationScope = keyof typeof keys;

export function invalidateFaceQueries(client: QueryClient, scope: FaceInvalidationScope): void {
  for (const key of keys[scope]) void client.invalidateQueries({ queryKey: [key] });
}
