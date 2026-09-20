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
  FaceModelDownloadProgress,
  FaceReviewItem,
  FaceReviewPage,
  FaceWorkbenchContext,
} from "@/types";

const api = vi.hoisted(() => ({
  setPersonTagPath: vi.fn().mockResolvedValue(undefined),
  capability: vi.fn(),
  persons: vi.fn(),
  clusters: vi.fn(),
  review: vi.fn(),
  createPerson: vi.fn(),
  decideFace: vi.fn(),
  clearFaceDecision: vi.fn(),
  setFaceClarity: vi.fn(),
  deletePerson: vi.fn(),
  renamePerson: vi.fn(),
  startFaceAnalysis: vi.fn(),
  cancelFaceAnalysis: vi.fn(),
  updateSettings: vi.fn(),
  installModel: vi.fn(),
  onProgress: vi.fn(),
  onModelProgress: vi.fn(),
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
    browseScope: { rootPath: "/photos", directory: "/photos/trip" },
  } as FaceWorkbenchContext,
}));

vi.mock("@/lib/api", () => ({
  listCustomTags: async () => [],
  setPersonTagPath: api.setPersonTagPath,
  getFaceSyncStatus: async () => [],
  clearFaceAnalysisData: async () => {},
  deletePeopleAnnotations: async () => {},
  resolveFaceSyncConflict: async () => {},
  cancelFaceAnalysis: api.cancelFaceAnalysis,
  clearFaceDecision: api.clearFaceDecision,
  setFaceClarity: api.setFaceClarity,
  closeFaceWorkbench: api.closeWindow,
  createPerson: api.createPerson,
  decideFace: api.decideFace,
  deletePerson: api.deletePerson,
  getFaceCapability: api.capability,
  installFaceModel: api.installModel,
  getFaceClusters: api.clusters,
  getFaceCrops: api.crops,
  getFaceReviewPage: api.review,
  getFaceCalibration: api.getFaceCalibration,
  getPersonUndo: api.getPersonUndo,
  listPersons: api.persons,
  mergePersons: api.mergePersons,
  notifyFaceAssetReveal: api.notifyReveal,
  onFaceAnalysisProgress: api.onProgress,
  onFaceModelDownloadProgress: api.onModelProgress,
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

vi.mock("./FacePhotoPreview", () => ({ FacePhotoPreview: () => <div data-testid="photo-preview" /> }));

const settings = {
  detectionConfidence: 0.5,
  nmsThreshold: 0.3,
  maxFacesPerAsset: 64,
  minFacePixels: 24,
  detectSmallFaces: false,
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
  facePixels: 52,
  clarity: 0.4,
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
  facePixels: 96,
  clarity: 0.8,
  state: "unknown",
};

const cluster: FaceCluster = {
  clusterId: "cluster-1",
  observationIds: ["face-2", "face-3", "face-4"],
  representativeObservationId: "face-2",
  memberCount: 3,
  cohesion: 0.71,
  outlierObservationIds: ["face-4"],
  memberQuality: [
    { observationId: "face-2", detectionScore: 0.95, facePixels: 100, clarity: 0.8 },
    { observationId: "face-3", detectionScore: 0.9, facePixels: 70, clarity: 0.6 },
    { observationId: "face-4", detectionScore: 0.85, facePixels: 40, clarity: 0.3 },
  ],
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
    models: [],
  });
  api.persons.mockResolvedValue([]);
  api.clusters.mockResolvedValue([cluster]);
  api.review.mockResolvedValue({ items: [pendingItem, unknownItem], total: 2, nextCursor: null } satisfies FaceReviewPage);
  api.createPerson.mockResolvedValue({ personId: "person-new", displayName: "Bob" });
  api.decideFace.mockResolvedValue(undefined);
  api.startFaceAnalysis.mockResolvedValue("job-1");
  api.onProgress.mockResolvedValue(() => {});
  api.onModelProgress.mockResolvedValue(() => {});
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

it("keeps analysis and review resources scoped to the selected folder", async () => {
  await render();
  await clickButton("peopleAnalyzeStart");
  expect(api.startFaceAnalysis).toHaveBeenCalledWith({
    rootPath: "/photos",
    directory: "/photos/trip",
  });
  expect(api.review).toHaveBeenCalledWith(0, 200, "/photos", "/photos/trip");
  expect(host.textContent).not.toContain("peopleAnalyzeSelection");
  expect(host.textContent).not.toContain("peopleAnalyzeLibrary");
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

it("adds committed detections to the workbench before the scan completes", async () => {
  const emitProgress = progressDriver();
  await render();
  const liveItem: FaceReviewItem = {
    ...unknownItem,
    observationId: "face-live",
    assetId: "asset-live",
    assetPath: "/photos/live.jpg",
  };
  api.review.mockResolvedValue({
    items: [pendingItem, unknownItem, liveItem],
    total: 3,
    nextCursor: null,
  } satisfies FaceReviewPage);

  await emitProgress({
    stage: "detecting",
    processedAssets: 1,
    totalAssets: 20,
    facesDetected: 1,
  });
  await settle();

  const livePhoto = [...host.querySelectorAll<HTMLElement>(".face-photo")]
    .find((photo) => photo.textContent?.includes("live.jpg"));
  expect(livePhoto).toBeDefined();
  const reject = [...livePhoto!.querySelectorAll<HTMLButtonElement>("button")]
    .find((button) => button.textContent?.includes("peopleNotFace"));
  await act(async () => reject!.click());
  await settle();
  expect(api.decideFace).toHaveBeenCalledWith("face-live", { decision: "notFace" });
});

it("does not load or analyze library resources without a selected folder", async () => {
  api.context = { locale: "zh-CN" };
  await render();
  const start = [...host.querySelectorAll<HTMLButtonElement>("button")].find((button) =>
    button.textContent?.includes("peopleAnalyzeStart"),
  )!;
  expect(start.disabled).toBe(true);
  expect(api.review).not.toHaveBeenCalled();
  expect(api.startFaceAnalysis).not.toHaveBeenCalled();
});

it("merges two names for the same person through the durable operation", async () => {
  api.persons.mockResolvedValue([
    { personId: "person-1", displayName: "Alice", createdAtMs: 1, updatedAtMs: 1, faceCount: 2 },
    { personId: "person-2", displayName: "Alica", createdAtMs: 1, updatedAtMs: 1, faceCount: 1 },
  ]);
  await render();
  await clickButton("peoplePersons");

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

it("offers a labelled undo for the newest person operation", async () => {
  api.getPersonUndo.mockResolvedValue({
    kind: "mergePersons",
    personId: "person-1",
    faceCount: 3,
    otherPersonName: "Alica",
  });
  await render();
  await clickButton("peoplePersons");
  const undo = [...host.querySelectorAll<HTMLButtonElement>(".face-photos__management button:last-child")][0];
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
  const primary = [...host.querySelectorAll<HTMLButtonElement>("button")].find((button) =>
    button.textContent?.includes("peopleAnalyzeStart"),
  )!;
  expect(primary.disabled).toBe(true);
});

it("downloads a missing face model only after its button is clicked", async () => {
  const capability = {
    available: false,
    running: false,
    settings,
    stats: { analyzedAssets: 0, facesDetected: 0, persons: 0, pendingReviews: 0, unknownFaces: 0, clusters: 0 },
    peopleStorePath: "demo/people.json",
    models: [
      { id: "scrfd-10g-kps", displayName: "SCRFD-10G KPS", installed: false, sizeBytes: 16_923_827, downloadSizeBytes: 288_621_354, licenseSummary: "research" },
      { id: "adaface-ir101", displayName: "AdaFace IR-101", installed: false, sizeBytes: 260_704_652, downloadSizeBytes: 260_704_652, licenseSummary: "experimental" },
    ],
  };
  api.capability.mockResolvedValue(capability);
  api.installModel.mockResolvedValue({
    ...capability,
    models: capability.models.map((model) => model.id === "scrfd-10g-kps" ? { ...model, installed: true } : model),
  });
  await render();
  const scrfd = [...host.querySelectorAll(".face-models__item")]
    .find((item) => item.textContent?.includes("SCRFD-10G KPS"))!;
  await act(async () => scrfd.querySelector<HTMLButtonElement>("button")!.click());
  await settle();
  expect(api.installModel.mock.calls[0][0]).toBe("scrfd-10g-kps");
});

it("shows live byte progress for the model being downloaded", async () => {
  let emitProgress: ((progress: FaceModelDownloadProgress) => void) | undefined;
  api.onModelProgress.mockImplementation(async (callback: (progress: FaceModelDownloadProgress) => void) => {
    emitProgress = callback;
    return () => {};
  });
  api.capability.mockResolvedValue({
    available: false,
    running: false,
    settings,
    stats: { analyzedAssets: 0, facesDetected: 0, persons: 0, pendingReviews: 0, unknownFaces: 0, clusters: 0 },
    peopleStorePath: "demo/people.json",
    models: [{ id: "scrfd-10g-kps", displayName: "SCRFD-10G KPS", installed: false, sizeBytes: 16_923_827, downloadSizeBytes: 100, licenseSummary: "research" }],
  });
  await render();
  await act(async () => emitProgress?.({ modelId: "scrfd-10g-kps", stage: "downloading", downloadedBytes: 42, totalBytes: 100 }));
  const indicator = host.querySelector<HTMLElement>(".face-models__progress")!;
  expect(indicator.getAttribute("aria-valuenow")).toBe("42");
  expect(indicator.textContent).toContain("42%");
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

it("starts with all photos and shares one card across faces in a photo", async () => {
  api.review.mockResolvedValue({ items: [pendingItem, { ...unknownItem, assetPath: pendingItem.assetPath }], total: 2, nextCursor: null });
  await render();
  expect(api.review).toHaveBeenCalledWith(0, 200, "/photos", "/photos/trip");
  expect(host.querySelectorAll(".face-photo")).toHaveLength(1);
  expect(host.querySelectorAll(".face-photo__face")).toHaveLength(2);
});

it("applies an action under a selected face to the entire selection", async () => {
  await render();
  const faces = [...host.querySelectorAll<HTMLInputElement>(".face-photo__select input")];
  await act(async () => faces[0].click());
  await act(async () => faces[1].dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true })));
  await clickButton("peopleNotFace");
  expect(api.decideFace.mock.calls).toEqual([
    [pendingItem.observationId, { decision: "notFace" }],
    [unknownItem.observationId, { decision: "notFace" }],
  ]);
});

it("does not apply an unselected card action to another selected face", async () => {
  await render();
  const faces = [...host.querySelectorAll<HTMLInputElement>(".face-photo__select input")];
  await act(async () => faces[1].click());
  await clickButton("peopleNotFace");
  expect(api.decideFace).toHaveBeenCalledTimes(1);
  expect(api.decideFace).toHaveBeenCalledWith(pendingItem.observationId, { decision: "notFace" });
});

it("clears selection when switching grouping", async () => {
  await render();
  await clickButton("facePhotosSelectLoaded");
  await clickButton("facePhotosView_similar");
  expect(host.querySelectorAll('.face-photo__face.is-selected')).toHaveLength(0);
  await clickButton("peopleNotFace");
  expect(api.decideFace).toHaveBeenCalledTimes(1);
});

it("moves a confirmed face to its person group after the durable write", async () => {
  api.review.mockResolvedValue({ items: [pendingItem], total: 1, nextCursor: null });
  api.assignFacesToPerson.mockImplementation(async () => {
    api.review.mockResolvedValue({ items: [{ ...pendingItem, state: "confirmed", confirmedPersonId: "person-1", confirmedPersonName: "Alice" }], total: 1, nextCursor: null });
    return 1;
  });
  await render();
  await clickButton("facePhotosView_status");
  expect(host.querySelector('.face-photos__group > header')?.textContent).toContain("peopleState_pending");
  await clickButton("peopleConfirmMatch");
  expect(host.querySelector('.face-photos__group > header')?.textContent).toContain("Alice");
});

it("returns a non-face to pending using the persisted decision reset", async () => {
  api.review.mockResolvedValue({ items: [{ ...unknownItem, state: "notFace" }], total: 1, nextCursor: null });
  await render();
  const button = [...host.querySelectorAll<HTMLButtonElement>(".face-photo__actions button")].find((button) => button.textContent === "peopleState_pending")!;
  await act(async () => button.click());
  expect(api.clearFaceDecision).toHaveBeenCalledWith(unknownItem.observationId);
});

it("refreshes after a partially failed batch and reports the failure", async () => {
  api.decideFace.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("disk full"));
  await render();
  await clickButton("facePhotosSelectLoaded");
  const before = api.review.mock.calls.length;
  await clickButton("peopleNotFace");
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("disk full");
  expect(api.review.mock.calls.length).toBeGreaterThan(before);
});

it("collapses groups, unmounts previews, and remembers the state per view", async () => {
  await render();
  await clickButton("facePhotosView_status");
  const toggle = host.querySelector<HTMLButtonElement>(".face-photos__group-toggle")!;
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  await act(async () => toggle.click());
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(host.querySelectorAll(".face-photo")).toHaveLength(0);
  expect(host.querySelector(".face-photos__group > header")?.textContent).toContain("2");
  await clickButton("facePhotosView_photos");
  expect(host.querySelectorAll(".face-photo")).toHaveLength(2);
  await clickButton("facePhotosView_status");
  expect(host.querySelectorAll(".face-photo")).toHaveLength(0);
  await act(async () => host.querySelector<HTMLButtonElement>(".face-photos__group-toggle")!.click());
  expect(host.querySelectorAll(".face-photo")).toHaveLength(2);
});

it("excludes collapsed groups from both an existing selection and select all", async () => {
  api.review.mockResolvedValue({ items: [pendingItem, confirmedItem], total: 2, nextCursor: null });
  await render();
  await clickButton("facePhotosView_status");
  await clickButton("facePhotosSelectLoaded");
  const toggles = [...host.querySelectorAll<HTMLButtonElement>(".face-photos__group-toggle")];
  await act(async () => toggles[0].click());
  await clickButton("facePhotosSelectLoaded");
  await clickButton("peopleNotFace");
  expect(api.decideFace).toHaveBeenCalledTimes(1);
  expect(api.decideFace).toHaveBeenCalledWith(confirmedItem.observationId, { decision: "notFace" });
  await act(async () => host.querySelector<HTMLButtonElement>(".face-photos__group-toggle")!.click());
  expect(host.querySelectorAll('.face-photo__face.is-selected')).toHaveLength(0);
});


it("manually marks all selected faces without changing their identities", async () => {
  await render();
  await clickButton("facePhotosSelectLoaded");
  await clickButton("facePhotosMarkBlurry");
  expect(api.setFaceClarity).toHaveBeenCalledWith([pendingItem.observationId, unknownItem.observationId], true);
  expect(api.decideFace).not.toHaveBeenCalled();
  expect(api.assignFacesToPerson).not.toHaveBeenCalled();
  await clickButton("facePhotosMarkClear");
  expect(api.setFaceClarity).toHaveBeenLastCalledWith([pendingItem.observationId], false);
  await clickButton("facePhotosQualityAuto");
  expect(api.setFaceClarity).toHaveBeenLastCalledWith([pendingItem.observationId], null);
});

it("uses manual quality for labels and blurry-only filtering", async () => {
  api.review.mockResolvedValue({ items: [{ ...pendingItem, clarity: 0.99, manualBlurry: true }, { ...unknownItem, clarity: 0.01, manualBlurry: false }], total: 2, nextCursor: null });
  await render();
  expect(host.textContent).toContain("facePhotosManualBlurry");
  expect(host.textContent).toContain("facePhotosManualClear");
  await act(async () => host.querySelector<HTMLInputElement>('.face-photos__toolbar input[type="checkbox"]')!.click());
  expect(host.querySelectorAll(".face-photo")).toHaveLength(1);
  expect(host.textContent).toContain("facePhotosManualBlurry");
  expect(host.textContent).not.toContain("facePhotosManualClear");
});

it("card checkboxes select all displayed faces and toggle without modifier keys", async () => {
  api.review.mockResolvedValue({ items: [pendingItem, { ...unknownItem, assetPath: pendingItem.assetPath }, confirmedItem], total: 3, nextCursor: null });
  await render();
  const boxes = [...host.querySelectorAll<HTMLInputElement>(".face-photo__select input")];
  expect(boxes).toHaveLength(2);
  expect(host.querySelector("button.face-photo__identity")).toBeNull();
  await act(async () => boxes[0].click());
  expect(host.querySelectorAll(".face-photo__face.is-selected")).toHaveLength(2);
  await act(async () => boxes[1].click());
  expect(host.querySelectorAll(".face-photo__face.is-selected")).toHaveLength(3);
  await act(async () => boxes[0].click());
  expect(host.querySelectorAll(".face-photo__face.is-selected")).toHaveLength(1);
  await clickButton("facePhotosMarkBlurry");
  // Action on an unselected card targets that card's first face, as before.
  expect(api.setFaceClarity).toHaveBeenCalledWith([pendingItem.observationId], true);
});

it("saves a person's hierarchical classification path", async () => {
  api.persons.mockResolvedValue([{ personId: "alice", displayName: "Alice", faceCount: 1, createdAtMs: 0, updatedAtMs: 0 }]);
  api.setPersonTagPath.mockResolvedValue(undefined);
  await render();
  await clickButton("peoplePersons");
  const input = host.querySelector<HTMLInputElement>('.people-person-path input')!;
  expect(input.value).toBe("人物 / Alice");
  await act(async () => setInputValue(input, "人物 / 家人 / Alice"));
  await act(async () => input.closest("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
  await settle();
  expect(api.setPersonTagPath).toHaveBeenCalledWith("alice", "人物|家人|Alice");
});
