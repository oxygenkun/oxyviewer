import { describe, expect, it } from "vitest";

import { faceRevealStep, type FaceRevealInput } from "./faceReveal";
import type { FaceAssetReveal, FolderSession } from "@/types";

const session: FolderSession = {
  id: "session-1",
  rootPath: "/photos",
  displayName: "Photos",
  openedAtMs: 1,
  deletionMode: "trash",
};

const reveal: FaceAssetReveal = {
  observationId: "face-1",
  assetId: "asset-9",
  assetPath: "/photos/trip/a.jpg",
};

const input = (overrides: Partial<FaceRevealInput> = {}): FaceRevealInput => ({
  reveal,
  sessions: [session],
  activeRoot: "/photos",
  currentDirectory: "/photos/trip",
  visibleAssetIds: ["asset-9"],
  directorySettled: true,
  filtersActive: false,
  ...overrides,
});

describe("face reveal navigation", () => {
  it("selects the asset once it is already visible", () => {
    expect(faceRevealStep(input())).toEqual({ kind: "select", assetId: "asset-9" });
  });

  it("navigates to the containing folder when another directory is open", () => {
    const step = faceRevealStep(input({ currentDirectory: "/photos/other", visibleAssetIds: [] }));
    expect(step).toEqual({ kind: "navigate", session, directory: "/photos/trip" });
  });

  it("navigates when the owning session is not the active root", () => {
    const step = faceRevealStep(input({ activeRoot: "/other", visibleAssetIds: [] }));
    expect(step).toEqual({ kind: "navigate", session, directory: "/photos/trip" });
  });

  it("waits while the directory still has pages to load", () => {
    expect(faceRevealStep(input({ visibleAssetIds: [], directorySettled: false })))
      .toEqual({ kind: "wait" });
  });

  it("reports a filtered grid once the directory is exhausted", () => {
    expect(faceRevealStep(input({ visibleAssetIds: [], filtersActive: true })))
      .toEqual({ kind: "unresolved", reason: "filtered" });
  });

  it("reports a missing asset when no filter explains it", () => {
    expect(faceRevealStep(input({ visibleAssetIds: [] })))
      .toEqual({ kind: "unresolved", reason: "notFound" });
  });

  it("rejects a path outside every opened library folder", () => {
    expect(faceRevealStep(input({ reveal: { ...reveal, assetPath: "/elsewhere/a.jpg" } })))
      .toEqual({ kind: "unresolved", reason: "outsideLibrary" });
  });

  it("never navigates to a directory that is not directly listable", () => {
    // The grid is non-recursive, so a session root one level below the asset's
    // parent cannot show it even though it contains it.
    const nested: FolderSession = { ...session, rootPath: "/photos/trip/a.jpg/deeper" };
    expect(faceRevealStep(input({ sessions: [nested] })))
      .toEqual({ kind: "unresolved", reason: "outsideLibrary" });
  });
});
