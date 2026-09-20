import { expect, it } from "vitest";
import type { FaceReviewItem } from "@/types";
import { actionTargets, groupFacePhotos, reviewStatus, selectFaceRange } from "./faceWorkbenchModel";
const face = (id: string, state: FaceReviewItem["state"] = "unknown"): FaceReviewItem => ({ observationId: id, assetId: "photo", assetPath: "/photo.jpg", state, detectionScore: 0.9, bbox: { x: 0, y: 0, width: 0.1, height: 0.2 } });
it("shows a group photo once by default and once per relevant status group", () => {
  const items = [{ ...face("a", "confirmed"), confirmedPersonId: "alice", confirmedPersonName: "Alice" }, face("b"), face("c", "notFace")];
  expect(groupFacePhotos(items, "photos", [])[0].cards[0].faces).toHaveLength(3);
  expect(groupFacePhotos(items, "status", []).map((group) => group.id)).toEqual(["pending", "person:alice", "notFace"]);
  expect(items[1].state).toBe("unknown");
});
it("a rejected identity remains pending, not a false-positive detection", () => {
  expect(reviewStatus(face("a", "rejected"))).toBe("pending");
});
it("range selection follows the displayed ordering in either direction", () => {
  expect(selectFaceRange(["a", "b", "c"], [], "a", "c", false, true)).toEqual(["a", "b", "c"]);
  expect(actionTargets("b", ["a"])).toEqual(["b"]);
});

it("orders status groups pending first and non-faces last regardless of incoming order", () => {
  const items = [face("n", "notFace"), { ...face("z", "confirmed"), confirmedPersonId: "z", confirmedPersonName: "Zoe" }, face("p"), { ...face("a", "confirmed"), confirmedPersonId: "a", confirmedPersonName: "Alice" }];
  expect(groupFacePhotos(items, "status", []).map((group) => group.id)).toEqual(["pending", "person:a", "person:z", "notFace"]);
});

it("separates pending similar groups from confirmed people and non-faces", () => {
  const items = [
    face("n", "notFace"),
    { ...face("c", "confirmed"), confirmedPersonId: "alice", confirmedPersonName: "Alice" },
    face("u"), face("p", "pending"),
  ];
  const clusters = [
    { clusterId: "a", observationIds: ["u"], representativeObservationId: "u", memberCount: 1, cohesion: 1, outlierObservationIds: [] },
    { clusterId: "z", observationIds: ["p", "c", "n"], representativeObservationId: "p", memberCount: 3, cohesion: 1, outlierObservationIds: [], suggestedName: "Alice" },
  ];
  const groups = groupFacePhotos(items, "similar", clusters);
  expect(groups.map((group) => group.id)).toEqual(["cluster:z", "cluster:a", "person:alice", "notFace"]);
  expect(groups[0].suggestedName).toBe("Alice");
  expect(groups.map((group) => group.cards[0].faces.map((item) => item.observationId))).toEqual([["p"], ["u"], ["c"], ["n"]]);
  expect(groupFacePhotos([...items].reverse(), "similar", clusters)).toEqual(groups);
});

it("keeps unrelated unclustered faces separate and uses candidate names as suggestions", () => {
  const items = [face("b"), { ...face("a"), candidate: { observationId: "a", personId: "alice", personName: "Alice", similarity: 0.9, matcherFingerprint: "matcher" } }];
  const groups = groupFacePhotos(items, "similar", []);
  expect(groups.map((group) => group.id)).toEqual(["unknown:a", "unknown:b"]);
  expect(groups[0].suggestedName).toBe("Alice");
  expect(groups[1].suggestedName).toBeUndefined();
});
