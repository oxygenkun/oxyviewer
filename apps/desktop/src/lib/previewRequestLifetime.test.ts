import { expect, it } from "vitest";
import { previewRequestSignal, retainPreviewRequest } from "./previewRequestLifetime";

it("cancels an Interim upgrade only when its last shared observer leaves", async () => {
  const releaseLoupe = retainPreviewRequest("shared");
  const releaseFilmstrip = retainPreviewRequest("shared");
  const signal = previewRequestSignal("shared", new AbortController().signal);
  releaseLoupe();
  await Promise.resolve();
  expect(signal.aborted).toBe(false);
  releaseFilmstrip();
  await Promise.resolve();
  expect(signal.aborted).toBe(true);
  const releaseReturn = retainPreviewRequest("shared");
  expect(previewRequestSignal("shared", new AbortController().signal).aborted).toBe(false);
  releaseReturn();
});

it("preserves an upgrade across StrictMode effect replacement", async () => {
  const release = retainPreviewRequest("strict");
  const signal = previewRequestSignal("strict", new AbortController().signal);
  release();
  const releaseAgain = retainPreviewRequest("strict");
  await Promise.resolve();
  expect(signal.aborted).toBe(false);
  releaseAgain();
  await Promise.resolve();
  expect(signal.aborted).toBe(true);
});
