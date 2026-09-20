// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import { FacePhotoPreview } from "./FacePhotoPreview";
const api = vi.hoisted(() => ({ preview: vi.fn(), renew: vi.fn(async () => true), release: vi.fn(async () => {}) }));
vi.mock("@/lib/api", () => ({ faceWorkbenchPreview: api.preview, renewMediaResource: api.renew, releaseMediaResource: api.release }));
afterEach(() => { vi.unstubAllGlobals(); vi.clearAllMocks(); });
it("only requests nearby photos and releases the resource when it leaves the viewport", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let observe!: (entries: { isIntersecting: boolean }[]) => void;
  vi.stubGlobal("IntersectionObserver", class { constructor(callback: typeof observe) { observe = callback; } observe() {} disconnect() {} });
  api.preview.mockResolvedValue({ path: "/a.jpg", url: "https://media.test/a.jpg", width: 160, height: 120,
    geometry: { displaySize: { width: 200, height: 400 }, contentRect: { x: 50, y: 0, width: 60, height: 120 } },
    resource: { resourceId: "photo-1", url: "https://media.test/a.jpg", mediaType: "image/jpeg" } });
  const host = document.createElement("div");
  const root = createRoot(host);
  const client = new QueryClient();
  try {
    await act(async () => root.render(<QueryClientProvider client={client}><FacePhotoPreview path="/a.jpg" faces={[{ observationId: "face-1", assetId: "a", assetPath: "/a.jpg", state: "unknown", detectionScore: 1, bbox: { x: 0.2, y: 0.1, width: 0.3, height: 0.4 } }]} selected={[]} onSelect={() => {}} unavailable="unavailable" /></QueryClientProvider>));
    expect(api.preview).not.toHaveBeenCalled();
    await act(async () => observe([{ isIntersecting: true }]));
    await act(async () => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(api.preview).toHaveBeenCalledTimes(1);
    const image = host.querySelector("img")!;
    await act(async () => image.dispatchEvent(new Event("load")));
    expect(host.querySelector<HTMLElement>(".face-photo__canvas")!.style.width).toBe("min(100%, 50cqh)");
    expect(host.querySelector<HTMLElement>(".face-photo__region")!.style.left).toBe("20%");
    expect(image.style.left).toBe("-83.33333333333334%");
    await act(async () => observe([{ isIntersecting: false }]));
    expect(host.querySelector("img")).toBeNull();
    expect(api.release).toHaveBeenCalledWith("photo-1");
  } finally { await act(async () => root.unmount()); client.clear(); }
});
