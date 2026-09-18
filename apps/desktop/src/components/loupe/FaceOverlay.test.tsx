// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { FaceOverlay } from "./FaceOverlay";
import type { AssetSummary, FaceReviewItem } from "@/types";

const api = vi.hoisted(() => ({
  reviews: vi.fn(),
  crops: vi.fn(),
  decideFace: vi.fn(),
  createPerson: vi.fn(),
  releaseResource: vi.fn(),
  renewResource: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  createPerson: api.createPerson,
  decideFace: api.decideFace,
  getAssetFaceReviews: api.reviews,
  getFaceCrops: api.crops,
  releaseMediaResource: api.releaseResource,
  renewMediaResource: api.renewResource,
}));

const asset = {
  id: "asset-1",
  path: "/photos/a.jpg",
  name: "a.jpg",
  extension: "jpg",
  kind: "jpeg",
  sizeBytes: 1,
  modifiedAtMs: 1,
  hasSidecar: false,
} as AssetSummary;

const pending: FaceReviewItem = {
  observationId: "face-1",
  assetId: "asset-1",
  assetPath: "/photos/a.jpg",
  bbox: { x: 0.25, y: 0.5, width: 0.2, height: 0.25 },
  detectionScore: 0.96,
  state: "pending",
  candidate: {
    observationId: "face-1",
    personId: "person-1",
    personName: "Alice",
    similarity: 0.55,
    matcherFingerprint: "matcher",
  },
};

const unknown: FaceReviewItem = {
  observationId: "face-2",
  assetId: "asset-1",
  assetPath: "/photos/a.jpg",
  bbox: { x: 0.6, y: 0.2, width: 0.15, height: 0.2 },
  detectionScore: 0.91,
  state: "unknown",
};

let host: HTMLDivElement;
let root: Root;
let client: QueryClient;

const settle = async () => {
  await act(async () => new Promise((resolve) => setTimeout(resolve, 20)));
};

const render = async (enabled = true) => {
  await act(async () =>
    root.render(
      <QueryClientProvider client={client}>
        <FaceOverlay asset={asset} enabled={enabled} t={(key) => key} />
      </QueryClientProvider>,
    ),
  );
  await settle();
};

const box = (observationId: string) =>
  host.querySelector<HTMLElement>(`[data-testid="face-box-${observationId}"]`)!;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  Object.values(api).forEach((mock) => mock.mockReset());
  api.reviews.mockResolvedValue([pending, unknown]);
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
  api.decideFace.mockResolvedValue(undefined);
  api.createPerson.mockResolvedValue({ personId: "person-new", displayName: "Bob", faceCount: 0 });
  api.releaseResource.mockResolvedValue(undefined);
  api.renewResource.mockResolvedValue(true);
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  host = document.createElement("div");
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  client.clear();
  vi.unstubAllGlobals();
});

it("draws a box per detected face from the stored normalized region", async () => {
  await render();
  const boxes = [...host.querySelectorAll<HTMLElement>(".loupe__face-box")];
  expect(boxes).toHaveLength(2);
  // Normalized regions become percentages, so the box is correct at any zoom.
  expect(box("face-1").style.left).toBe("25%");
  expect(box("face-1").style.top).toBe("50%");
  expect(box("face-1").style.width).toBe("20%");
  expect(box("face-1").style.height).toBe("25%");
  expect(box("face-1").className).toContain("is-pending");
  expect(box("face-2").className).toContain("is-unknown");
});

it("confirms or corrects a proposed match on the photo itself", async () => {
  await render();
  const anchor = box("face-1").querySelector<HTMLButtonElement>(".loupe__face-anchor")!;
  await act(async () => anchor.click());
  await settle();
  const buttons = [...box("face-1").querySelectorAll<HTMLButtonElement>(".loupe__face-actions button")];
  expect(buttons.length).toBeGreaterThanOrEqual(3);

  await act(async () => buttons[0].click());
  expect(api.decideFace).toHaveBeenCalledWith("face-1", {
    decision: "confirmPerson",
    personId: "person-1",
  });

  await act(async () => buttons[1].click());
  expect(api.decideFace).toHaveBeenCalledWith("face-1", {
    decision: "rejectPerson",
    personId: "person-1",
  });

  await act(async () => buttons[2].click());
  expect(api.decideFace).toHaveBeenCalledWith("face-1", { decision: "notFace" });
});

it("names an unknown face without leaving the loupe", async () => {
  await render();
  const anchor = box("face-2").querySelector<HTMLButtonElement>(".loupe__face-anchor")!;
  await act(async () => anchor.click());
  await settle();

  const input = box("face-2").querySelector<HTMLInputElement>(".loupe__face-actions input")!;
  const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    setValue.call(input, "Bob");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();

  const form = box("face-2").querySelector<HTMLFormElement>(".loupe__face-actions form")!;
  await act(async () => form.requestSubmit());
  await settle();

  expect(api.createPerson).toHaveBeenCalledTimes(1);
  expect(api.createPerson.mock.calls[0][1]).toBe("Bob");
  expect(api.decideFace).toHaveBeenCalledWith("face-2", {
    decision: "confirmPerson",
    personId: "person-new",
  });
});

it("renders nothing when boxes are hidden or the asset has no faces", async () => {
  await render(false);
  expect(host.querySelector(".loupe__face-overlay")).toBeNull();

  api.reviews.mockResolvedValue([]);
  await render(true);
  expect(host.querySelector(".loupe__face-overlay")).toBeNull();
});

it("keeps the Host lease on every crop it displays", async () => {
  await render();
  // Registering a resource leases it for the publish grace only, and a local
  // retain does not extend it: without an explicit renewal the crops become
  // blank squares as soon as the Host drops the handle.
  await settle();
  expect(api.renewResource).toHaveBeenCalledWith("resource-face-1");
  expect(api.renewResource).toHaveBeenCalledWith("resource-face-2");
});

it("registers the crops again when the Host has already dropped them", async () => {
  api.renewResource.mockResolvedValue(false);
  await render();
  await settle();

  // Retrying the immutable URL can never succeed, so the only recovery is a
  // fresh registration from the Host — exactly once, not in a loop.
  expect(api.crops.mock.calls.length).toBeGreaterThan(1);
  expect(api.crops.mock.calls.length).toBeLessThanOrEqual(2);
  expect(api.renewResource).toHaveBeenCalled();
});
