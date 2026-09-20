import type { FaceCluster, FaceReviewItem } from "@/types";

export type FaceView = "photos" | "status" | "similar";
export type ReviewStatus = "pending" | "confirmed" | "notFace";
export interface PhotoCard { path: string; faces: FaceReviewItem[] }
export interface PhotoGroup { id: string; name?: string; status: ReviewStatus; suggestedName?: string; cards: PhotoCard[] }

export function reviewStatus(face: FaceReviewItem): ReviewStatus {
  return face.state === "confirmed" ? "confirmed" : face.state === "notFace" ? "notFace" : "pending";
}

/** Group membership is a projection, never an identity decision. */
export function groupFacePhotos(items: FaceReviewItem[], view: FaceView, clusters: FaceCluster[]): PhotoGroup[] {
  const memberships = new Map<string, FaceCluster>();
  for (const cluster of clusters) for (const id of cluster.observationIds) memberships.set(id, cluster);
  const groups = new Map<string, { name?: string; status: ReviewStatus; suggestions: Set<string>; photos: Map<string, PhotoCard> }>();
  for (const face of items) {
    const status = reviewStatus(face);
    const cluster = memberships.get(face.observationId);
    const clusterId = cluster?.clusterId ?? face.clusterId;
    const id = view === "photos" ? "all" : status === "confirmed"
      ? `person:${face.confirmedPersonId}` : status === "notFace" ? "notFace"
      : view === "status" ? "pending" : clusterId ? `cluster:${clusterId}` : `unknown:${face.observationId}`;
    let group = groups.get(id);
    if (!group) {
      group = { name: view !== "photos" && status === "confirmed" ? face.confirmedPersonName : undefined, status, suggestions: new Set(), photos: new Map() };
      groups.set(id, group);
    }
    const suggestedName = cluster?.suggestedName ?? face.candidate?.personName;
    if (status === "pending" && suggestedName) group.suggestions.add(suggestedName);
    let card = group.photos.get(face.assetPath);
    if (!card) { card = { path: face.assetPath, faces: [] }; group.photos.set(face.assetPath, card); }
    if (!card.faces.some((item) => item.observationId === face.observationId)) card.faces.push(face);
  }
  const result = [...groups].map(([id, group]) => ({
    id, name: group.name, status: group.status,
    suggestedName: group.suggestions.size === 1 ? [...group.suggestions][0] : undefined,
    cards: [...group.photos.values()].sort((a, b) => a.path.localeCompare(b.path)),
  }));
  if (view !== "photos") {
    const rank = (group: PhotoGroup) => group.status === "pending"
      ? view === "similar" && !group.suggestedName ? 1 : 0 : group.status === "confirmed" ? 2 : 3;
    result.sort((a, b) => rank(a) - rank(b)
      || (a.name ?? a.suggestedName ?? "").localeCompare(b.name ?? b.suggestedName ?? "") || a.id.localeCompare(b.id));
  }
  return result;
}

export function actionTargets(faceId: string, selected: string[]): string[] {
  return selected.includes(faceId) ? selected : [faceId];
}

export function selectFaceRange(ids: string[], selected: string[], id: string, anchor: string | undefined, toggle: boolean, range: boolean): string[] {
  const start = anchor ? ids.indexOf(anchor) : -1;
  const end = ids.indexOf(id);
  if (range && start >= 0 && end >= 0) return ids.slice(Math.min(start, end), Math.max(start, end) + 1);
  if (toggle) return selected.includes(id) ? selected.filter((value) => value !== id) : [...selected, id];
  return [id];
}

/** Explicit false (clear) overrides a low model score too. */
export function faceIsBlurry(face: FaceReviewItem, threshold: number): boolean | undefined {
  return face.manualBlurry ?? (face.clarity === undefined ? undefined : face.clarity < threshold);
}
