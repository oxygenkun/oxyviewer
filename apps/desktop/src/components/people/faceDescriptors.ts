import type { MessageKey } from "@/lib/i18n";
import type { CollectionViewDescriptor, SettingsDescriptor } from "@/types";

export function faceSettingsDescriptor(t: (key: MessageKey) => string): SettingsDescriptor {
  return { id: "faces.settings", fields: [
    { key: "matchSensitivity", label: t("peopleMatchSensitivity"), type: "enum", values: ["strict", "balanced", "loose", "custom"], recompute: "matching" },
    { key: "matchThreshold", label: t("faceMatchThreshold"), type: "number", min: -1, max: 1, step: 0.001, recompute: "matching" },
    { key: "detectionConfidence", label: t("peopleDetectionConfidence"), type: "number", min: 0.01, max: 0.99, step: 0.01, recompute: "detection" },
    { key: "nmsThreshold", label: t("faceNmsThreshold"), type: "number", min: 0, max: 1, step: 0.01, recompute: "detection" },
    { key: "maxFacesPerAsset", label: t("faceMaxFaces"), type: "number", min: 1, max: 512, step: 1, recompute: "detection" },
    { key: "minFacePixels", label: t("faceMinPixels"), type: "number", min: 8, max: 512, step: 1, recompute: "detection" },
    { key: "detectSmallFaces", label: t("peopleDetectSmallFaces"), type: "boolean", recompute: "detection" },
    { key: "clusterThreshold", label: t("peopleClusterThreshold"), type: "number", min: 0, max: 1, step: 0.001, recompute: "clustering" },
  ] };
}
export const faceReviewDescriptor: CollectionViewDescriptor = {
  id: "faces.review", source: "faces.review", selection: "multi",
  fields: [], actions: ["faces.notFace"],
};
