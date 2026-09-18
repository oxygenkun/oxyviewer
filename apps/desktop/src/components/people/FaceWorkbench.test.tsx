// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { FaceWorkbench } from "./FaceWorkbench";
import type {
  FaceAnalysisProgress,
  FaceAssetReveal,
  FaceCluster,
  FaceReviewItem,
  FaceReviewPage,
  FaceWorkbenchContext,
} from "@/types";

const api = vi.hoisted(() => ({
  capability: vi.fn(),
  persons: vi.fn(),
  clusters: vi.fn(),
  review: vi.fn(),
  createPerson: vi.fn(),
  decideFace: vi.fn(),
  clearFaceDecision: vi.fn(),
  deletePerson: vi.fn(),
  renamePerson: vi.fn(),
  startFaceAnalysis: vi.fn(),
  cancelFaceAnalysis: vi.fn(),
  updateSettings: vi.fn(),
  onProgress: vi.fn(),
  onLibrary: vi.fn(),
  onContext: vi.fn(),
  requestContext: vi.fn(),
  closeWindow: vi.fn(),
  notifyReveal: vi.fn(),
  resolveObservation: vi.fn(),
  crops: vi.fn(),
  releaseResource: vi.fn(),
  renewResource: vi.fn(),
  getPersonUndo: vi.fn(),
  getFaceCalibration: vi.fn(),
  undoPersonOperation: vi.fn(),
  mergePersons: vi.fn(),
  removeFacesFromPerson: vi.fn(),
  assignFacesToPerson: vi.fn(),
  context: {
    locale: "zh-CN",
    visiblePaths: ["/photos/a.jpg"],
    browseScope: { rootPath: "/photos", directory: "/photos/trip" },
  } as FaceWorkbenchContext,
}));

vi.mock("@/lib/api", () => ({
  cancelFaceAnalysis: api.cancelFaceAnalysis,
  clearFaceDecision: api.clearFaceDecision,
  closeFaceWorkbench: api.closeWindow,
  createPerson: api.createPerson,
  decideFace: api.decideFace,
  deletePerson: api.deletePerson,
  getFaceCapability: api.capability,
  getFaceClusters: api.clusters,
  getFaceCrops: api.crops,
  getFaceReviewPage: api.review,
  getFaceCalibration: api.getFaceCalibration,
  getPersonUndo: api.getPersonUndo,
  listPersons: api.persons,
  mergePersons: api.mergePersons,
  notifyFaceAssetReveal: api.notifyReveal,
  onFaceAnalysisProgress: api.onProgress,
  onFaceLibraryUpdated: api.onLibrary,
  onFaceWorkbenchContext: api.onContext,
  removeFacesFromPerson: api.removeFacesFromPerson,
  assignFacesToPerson: api.assignFacesToPerson,
  requestFaceWorkbenchContext: api.requestContext,
  resolveFaceObservation: api.resolveObservation,
  releaseMediaResource: api.releaseResource,
  renewMediaResource: api.renewResource,
  renamePerson: api.renamePerson,
  startFaceAnalysis: api.startFaceAnalysis,
  undoPersonOperation: api.undoPersonOperation,
  updateFaceAnalyzerSettings: api.updateSettings,
}));

const settings = {
  detectionConfidence: 0.9,
  nmsThreshold: 0.3,
  maxFacesPerAsset: 64,
  minFacePixels: 24,
  detectSmallFaces: true,
  matchSensitivity: "balanced" as const,
  matchThreshold: 0.363,
  clusterThreshold: 0.363,
};

const pendingItem: FaceReviewItem = {
  observationId: "face-1",
  assetId: "asset-1",
  assetPath: "/photos/a.jpg",
  bbox: { x: 0.1, y: 0.1, width: 0.2, height: 0.2 },
  detectionScore: 0.97,
  state: "pending",
  candidate: {
    observationId: "face-1",
    personId: "person-1",
    personName: "Alice",
    similarity: 0.52,
    matcherFingerprint: "matcher",
  },
};

const unknownItem: FaceReviewItem = {
  observationId: "face-2",
  assetId: "asset-2",
  assetPath: "/photos/b.jpg",
  bbox: { x: 0.4, y: 0.2, width: 0.2, height: 0.2 },
  detectionScore: 0.93,
  state: "unknown",
};

const cluster: FaceCluster = {
  clusterId: "cluster-1",
  observationIds: ["face-2", "face-3", "face-4"],
  representativeObservationId: "face-2",
  memberCount: 3,
  cohesion: 0.71,
  outlierObservationIds: ["face-4"],
};

const confirmedItem: FaceReviewItem = {
  observationId: "face-9",
  assetId: "asset-9",
  assetPath: "/photos/c.jpg",
  bbox: { x: 0.2, y: 0.2, width: 0.2, height: 0.2 },
  detectionScore: 0.95,
  state: "confirmed",
  confirmedPersonId: "person-1",
  confirmedPersonName: "Alice",
};

let host: HTMLDivElement;
let root: Root;
let client: QueryClient;

const settle = async () => {
  await act(async () => new Promise((resolve) => setTimeout(resolve, 20)));
};

const render = async () => {
  await act(async () =>
    root.render(
      <QueryClientProvider client={client}>
        <FaceWorkbench t={(key) => key} />
      </QueryClientProvider>,
    ),
  );
  await settle();
};

/** Renders with the published locale, so interpolated copy can be asserted. */
const renderTranslated = async () => {
  await act(async () =>
    root.render(
      <QueryClientProvider client={client}>
        <FaceWorkbench />
      </QueryClientProvider>,
    ),
  );
  await settle();
};

/**
 * Captures the progress channel so a run can be driven to completion.
 *
 * Call before rendering: the workbench subscribes on mount.
 */
const progressDriver = () => {
  let emit: ((progress: FaceAnalysisProgress) => void) | undefined;
  api.onProgress.mockImplementation(
    async (callback: (progress: FaceAnalysisProgress) => void) => {
      emit = callback;
      return () => {};
    },
  );
  return (progress: Partial<FaceAnalysisProgress>) =>
    act(async () => {
      emit!({
        jobId: "job-1",
        stage: "complete",
        processedAssets: 0,
        totalAssets: 0,
        facesDetected: 0,
        pendingReviews: 0,
        failedAssets: 0,
        ...progress,
      });
    });
};

const clickTab = async (label: string) => {
  const tab = [...host.querySelectorAll<HTMLButtonElement>("[role=tab]")]
    .find((button) => button.textContent?.includes(label));
  expect(tab, label).toBeDefined();
  await act(async () => tab!.click());
  await settle();
};

const clickButton = async (text: string) => {
  const button = [...host.querySelectorAll<HTMLButtonElement>("button")]
    .find((candidate) => candidate.textContent?.includes(text));
  expect(button, text).toBeDefined();
  await act(async () => button!.click());
  await settle();
};

const setInputValue = (input: HTMLInputElement, value: string) => {
  const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  setValue.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  Object.values(api).forEach((mock) => {
    if (typeof mock === "function" && "mockReset" in mock) mock.mockReset();
  });
  api.capability.mockResolvedValue({
    available: true,
    running: false,
    settings,
    stats: { analyzedAssets: 2, facesDetected: 2, persons: 1, pendingReviews: 1, unknownFaces: 1, clusters: 1 },
    peopleStorePath: "demo/people.json",
  });
  api.persons.mockResolvedValue([]);
  api.clusters.mockResolvedValue([cluster]);
  api.review.mockResolvedValue({ items: [pendingItem, unknownItem], total: 2, nextCursor: null } satisfies FaceReviewPage);
  api.createPerson.mockResolvedValue({ personId: "person-new", displayName: "Bob" });
  api.decideFace.mockResolvedValue(undefined);
  api.startFaceAnalysis.mockResolvedValue("job-1");
  api.onProgress.mockResolvedValue(() => {});
  api.onLibrary.mockResolvedValue(() => {});
  // Registration delivers the current context, which is how the real window
  // learns the main window's scope without a shared store.
  api.onContext.mockImplementation(async (callback: (context: FaceWorkbenchContext) => void) => {
    callback(api.context);
    return () => {};
  });
  api.requestContext.mockResolvedValue(undefined);
  api.closeWindow.mockResolvedValue(undefined);
  api.notifyReveal.mockResolvedValue(undefined);
  api.resolveObservation.mockResolvedValue({
    observationId: "face-2",
    assetId: "asset-2",
    assetPath: "/photos/b.jpg",
  } satisfies FaceAssetReveal);
  api.crops.mockImplementation(async (ids: string[]) =>
    ids.map((observationId) => ({
      observationId,
      descriptor: {
        resourceId: `resource-${observationId}`,
        url: `oxy-media://localhost/resource/${observationId}`,
        mediaType: "image/jpeg",
      },
    })),
  );
  api.releaseResource.mockResolvedValue(undefined);
  api.renewResource.mockResolvedValue(true);
  api.getFaceCalibration.mockResolvedValue({
    acceptedScores: [],
    rejectedScores: [],
    currentThreshold: 0.363,
    separable: false,
  });
  api.getPersonUndo.mockResolvedValue(null);
  api.undoPersonOperation.mockResolvedValue(null);
  api.mergePersons.mockResolvedValue(0);
  api.removeFacesFromPerson.mockResolvedValue(0);
  api.assignFacesToPerson.mockResolvedValue(0);
  api.context = {
    locale: "zh-CN",
    visiblePaths: ["/photos/a.jpg"],
    browseScope: { rootPath: "/photos", directory: "/photos/trip" },
  };
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  host = document.createElement("div");
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  client.clear();
  vi.unstubAllGlobals();
});

it("scopes 'analyze this folder' to the directory, not the loaded pages", async () => {
  await render();
  const folder = [...host.querySelectorAll<HTMLButtonElement>("button")].find((button) =>
    button.textContent?.includes("peopleAnalyzeFolder"),
  )!;
  expect(folder.disabled).toBe(false);
  await act(async () => folder.click());
  // One asset is loaded, but the directory may hold hundreds: the run must be
  // scoped by folder so the background job enumerates what the grid cannot.
  expect(api.startFaceAnalysis).toHaveBeenCalledWith({
    rootPath: "/photos",
    directory: "/photos/trip",
  });

  await clickButton("peopleAnalyzeSelection");
  expect(api.startFaceAnalysis).toHaveBeenCalledWith({ paths: ["/photos/a.jpg"] });

  await clickButton("peopleAnalyzeLibrary");
  expect(api.startFaceAnalysis).toHaveBeenCalledWith({ force: false });
});

it("starts the big launch button on the folder scope when one is known", async () => {
  await render();
  await clickButton("peopleAnalyzeStart");
  expect(api.startFaceAnalysis).toHaveBeenCalledWith({
    rootPath: "/photos",
    directory: "/photos/trip",
  });
});

it("keeps the progress meter still until a run actually starts", async () => {
  await render();
  const meter = () => host.querySelector(".face-launch__meter")!;
  // An idle window must not look like work in progress.
  expect(meter().classList.contains("is-indeterminate")).toBe(false);
  expect(host.textContent).toContain("faceWorkbenchProgressIdle");

  api.capability.mockResolvedValue({
    available: true,
    running: true,
    settings,
    stats: { analyzedAssets: 0, facesDetected: 0, persons: 0, pendingReviews: 0, unknownFaces: 0, clusters: 0 },
    progress: {
      jobId: "job-1",
      stage: "detecting",
      processedAssets: 0,
      totalAssets: 0,
      facesDetected: 0,
      pendingReviews: 0,
    },
    peopleStorePath: "demo/people.json",
  });
  await act(async () => client.invalidateQueries());
  await settle();
  // Only a run whose total is still unknown earns the indeterminate sweep.
  expect(meter().classList.contains("is-indeterminate")).toBe(true);
});

it("falls back to the whole library when no folder scope was published", async () => {
  api.context = { locale: "zh-CN", visiblePaths: [] };
  await render();
  const selection = [...host.querySelectorAll<HTMLButtonElement>("button")].find((button) =>
    button.textContent?.includes("peopleAnalyzeSelection"),
  )!;
  expect(selection.disabled).toBe(true);
  await clickButton("peopleAnalyzeStart");
  expect(api.startFaceAnalysis).toHaveBeenCalledWith({ force: false });
});

it("answers a pending candidate with confirm, correct, and not-a-face", async () => {
  await render();
  await clickTab("peopleReview");
  // The identity translator keeps placeholder text, so the badge is asserted
  // by key; the candidate's identity is asserted through the decision payload.
  expect(host.textContent).toContain("peopleCandidate");
  expect(host.textContent).toContain("peopleState_pending");

  const buttons = () => [...host.querySelectorAll<HTMLButtonElement>(".people-panel__review-actions button")];
  await act(async () => buttons()[0].click());
  expect(api.decideFace).toHaveBeenCalledWith("face-1", { decision: "confirmPerson", personId: "person-1" });

  await act(async () => buttons()[1].click());
  expect(api.decideFace).toHaveBeenCalledWith("face-1", { decision: "rejectPerson", personId: "person-1" });

  // The unknown row has only the not-a-face action; index 2 is its button.
  const unknownButtons = [...host.querySelectorAll<HTMLButtonElement>(".people-panel__review li")][1]
    .querySelectorAll<HTMLButtonElement>(".people-panel__review-actions button");
  await act(async () => unknownButtons[0].click());
  expect(api.decideFace).toHaveBeenCalledWith("face-2", { decision: "notFace" });
});

it("naming a cluster confirms its members but leaves outliers unconfirmed", async () => {
  await render();
  const input = host.querySelector<HTMLInputElement>(".people-panel__cluster-actions input")!;
  await act(async () => setInputValue(input, "Bob"));
  await settle();

  await clickButton("peopleConfirmCluster");

  expect(api.createPerson).toHaveBeenCalledTimes(1);
  expect(api.createPerson.mock.calls[0][1]).toBe("Bob");
  const confirmed = api.decideFace.mock.calls.map((call) => call[0]);
  expect(confirmed).toEqual(["face-2", "face-3"]);
  expect(confirmed).not.toContain("face-4");
});

it("shows a crop for every face it asks the user to judge", async () => {
  await render();
  const crops = [...host.querySelectorAll<HTMLImageElement>("img.face-crop")];
  expect(crops.length).toBeGreaterThan(0);
  for (const crop of crops) {
    expect(crop.src).toContain("oxy-media://localhost/resource/");
  }
  expect(api.crops).toHaveBeenCalled();
  // The cluster's boundary member is marked, so the reviewer knows which face
  // the cluster is unsure about before confirming the group.
  const outliers = [...host.querySelectorAll(".face-crop-frame.is-outlier")];
  expect(outliers).toHaveLength(1);
  expect(outliers[0].querySelector("img")?.getAttribute("src")).toContain("face-4");
});

it("shows completed face batches while later Full crops are still loading", async () => {
  api.clusters.mockResolvedValue([{
    ...cluster,
    observationIds: ["face-2", "face-3", "face-4", "face-5", "face-6", "face-7"],
    memberCount: 6,
  }]);
  let finishSecondBatch!: () => void;
  const secondBatch = new Promise<void>((resolve) => { finishSecondBatch = resolve; });
  api.crops.mockImplementation(async (ids: string[]) => {
    if (ids.includes("face-6")) await secondBatch;
    return ids.map((observationId) => ({
      observationId,
      descriptor: {
        resourceId: `resource-${observationId}`,
        url: `oxy-media://localhost/resource/${observationId}`,
        mediaType: "image/jpeg",
      },
    }));
  });

  await render();
  expect(host.querySelector('img[src*="face-2"]')).not.toBeNull();
  expect(host.querySelector('img[src*="face-6"]')).toBeNull();
  await act(async () => finishSecondBatch());
  await settle();
  expect(host.querySelector('img[src*="face-6"]')).not.toBeNull();
});

it("reveals a clicked cluster face in the main window", async () => {
  await render();
  const crop = host.querySelector<HTMLButtonElement>(".face-crop-frame--action")!;
  await act(async () => crop.click());
  await settle();

  expect(api.resolveObservation).toHaveBeenCalled();
  expect(api.notifyReveal).toHaveBeenCalledWith({
    observationId: "face-2",
    assetId: "asset-2",
    assetPath: "/photos/b.jpg",
  });
  expect(host.querySelector(".face-workbench__notice")?.textContent).toContain("faceWorkbenchRevealed");
});

it("explains when a clicked face no longer resolves", async () => {
  api.resolveObservation.mockResolvedValue(null);
  await render();
  const crop = host.querySelector<HTMLButtonElement>(".face-crop-frame--action")!;
  await act(async () => crop.click());
  await settle();

  expect(api.notifyReveal).not.toHaveBeenCalled();
  expect(host.querySelector(".face-workbench__notice")?.textContent).toContain("faceWorkbenchRevealMissing");
});

it("merges two names for the same person through the durable operation", async () => {
  api.persons.mockResolvedValue([
    { personId: "person-1", displayName: "Alice", createdAtMs: 1, updatedAtMs: 1, faceCount: 2 },
    { personId: "person-2", displayName: "Alica", createdAtMs: 1, updatedAtMs: 1, faceCount: 1 },
  ]);
  await render();
  await clickTab("peoplePersons");

  const selects = [...host.querySelectorAll<HTMLSelectElement>(".people-panel__persons ~ * select")];
  const [source, target] = selects.length >= 2 ? selects : [...host.querySelectorAll<HTMLSelectElement>("select")].slice(-2);
  const setValue = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!;
  await act(async () => {
    setValue.call(source, "person-2");
    source.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await settle();
  await act(async () => {
    setValue.call(target, "person-1");
    target.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await settle();

  expect(api.mergePersons).toHaveBeenCalledWith("person-2", "person-1");
});

it("detaches a wrongly attached face without touching the rest of the person", async () => {
  api.review.mockResolvedValue({ items: [confirmedItem], total: 1, nextCursor: null });
  await render();
  await clickTab("peopleReview");

  const detach = [...host.querySelectorAll<HTMLButtonElement>(".people-panel__review-actions button")]
    .find((button) => button.textContent?.includes("peopleDetach"))!;
  expect(detach).toBeDefined();
  await act(async () => detach.click());
  expect(api.removeFacesFromPerson).toHaveBeenCalledWith("person-1", ["face-9"]);
});

it("offers a labelled undo for the newest person operation", async () => {
  api.getPersonUndo.mockResolvedValue({
    kind: "mergePersons",
    personId: "person-1",
    faceCount: 3,
    otherPersonName: "Alica",
  });
  await render();
  await clickTab("peoplePersons");
  const undo = [...host.querySelectorAll<HTMLButtonElement>(".people-panel__undo button")][0];
  expect(undo.textContent).toContain("peopleUndoMerge");
  await act(async () => undo.click());
  expect(api.undoPersonOperation).toHaveBeenCalled();
});

it("offers a threshold calibrated from the user's own decisions", async () => {
  api.getFaceCalibration.mockResolvedValue({
    acceptedScores: [0.7, 0.65, 0.6],
    rejectedScores: [0.4, 0.35, 0.3],
    currentThreshold: 0.363,
    recommendedThreshold: 0.5,
    separable: true,
  });
  await render();
  await clickButton("peopleParameters");
  const panel = host.querySelector(".people-panel__calibration")!;
  // The identity translator keeps placeholders, so the recommendation is
  // asserted through the payload the apply button sends.
  expect(panel.textContent).toContain("peopleCalibrationSamples");
  expect(panel.textContent).toContain("peopleCalibrationRecommended");

  const apply = panel.querySelector<HTMLButtonElement>("button")!;
  await act(async () => apply.click());
  // React Query appends its own context argument, so assert the payload only.
  expect(api.updateSettings.mock.calls[0][0]).toEqual({
    ...settings,
    matchSensitivity: "custom",
    matchThreshold: 0.5,
  });
});

it("stays silent while there is too little evidence", async () => {
  api.getFaceCalibration.mockResolvedValue({
    acceptedScores: [0.7],
    rejectedScores: [],
    currentThreshold: 0.363,
    separable: false,
  });
  await render();
  await clickButton("peopleParameters");
  const panel = host.querySelector(".people-panel__calibration")!;
  expect(panel.textContent).toContain("peopleCalibrationInsufficient");
  expect(panel.querySelector("button")).toBeNull();
});

it("disables analysis when no face models are installed", async () => {
  api.capability.mockResolvedValue({
    available: false,
    unavailableReason: "models missing",
    running: false,
    settings,
    stats: { analyzedAssets: 0, facesDetected: 0, persons: 0, pendingReviews: 0, unknownFaces: 0, clusters: 0 },
    peopleStorePath: "demo/people.json",
  });
  await render();
  expect(host.textContent).toContain("models missing");
  const analyze = [...host.querySelectorAll<HTMLButtonElement>("button")].find((button) =>
    button.textContent?.includes("peopleAnalyzeLibrary"),
  )!;
  expect(analyze.disabled).toBe(true);
  const primary = [...host.querySelectorAll<HTMLButtonElement>("button")].find((button) =>
    button.textContent?.includes("peopleAnalyzeStart"),
  )!;
  expect(primary.disabled).toBe(true);
});

it("opens the review queue on every unanswered face, not only matcher proposals", async () => {
  await render();
  await clickTab("peopleReview");
  // A library with no named people has no proposals at all, so a `pending`
  // default would show an empty queue right after analyzing 990 photos.
  expect(api.review).toHaveBeenCalledWith("unreviewed");

  await clickButton("peopleFilter_pending");
  expect(api.review).toHaveBeenCalledWith("pending");
  await clickButton("peopleFilter_unknown");
  expect(api.review).toHaveBeenCalledWith("unknown");
});

it("reports how many faces the last run found, not just how many files it touched", async () => {
  const emit = progressDriver();
  await renderTranslated();
  await emit({ processedAssets: 990, totalAssets: 990, facesDetected: 12, failedAssets: 0 });

  const summary = host.querySelector(".face-launch__summary")!;
  expect(summary.className).toContain("is-ok");
  expect(summary.textContent).toContain("990");
  expect(summary.textContent).toContain("12");
});

it("warns when a finished run found no faces or skipped files", async () => {
  const emit = progressDriver();
  await render();
  await emit({ processedAssets: 990, totalAssets: 990, facesDetected: 0, failedAssets: 7 });

  const summary = host.querySelector(".face-launch__summary")!;
  expect(summary.className).toContain("is-warn");
  expect(summary.textContent).toContain("faceWorkbenchRunNoFaces");
  expect(summary.textContent).toContain("faceWorkbenchRunFailures");
});

it("shows why a run failed instead of leaving the panel empty", async () => {
  const emit = progressDriver();
  await renderTranslated();
  await emit({ stage: "failed", message: "clustering input exceeds the limit" });

  const summary = host.querySelector(".face-launch__summary")!;
  // The failure used to be published and then dropped on the floor: the panel
  // showed nothing at all.
  expect(summary.className).toContain("is-error");
  expect(summary.textContent).toContain("clustering input exceeds the limit");
});
