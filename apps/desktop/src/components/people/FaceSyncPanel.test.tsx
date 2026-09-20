// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { FaceSyncPanel } from "./FaceSyncPanel";

const api = vi.hoisted(() => ({ resolve: vi.fn(async () => {}), clear: vi.fn(async () => {}) }));
vi.mock("@/lib/api", () => ({
  resolveFaceSyncConflict: api.resolve,
  clearFaceAnalysisData: api.clear,
  deletePeopleAnnotations: vi.fn(),
  getFaceSyncStatus: async () => [{
    path: "/photos/portrait.jpg", pending: true, conflict: true,
    localFacts: { facts: [{ id: "fact", revision: "local", region: { x: 0.1, y: 0.2, width: 0.2, height: 0.2 }, decision: { decision: "confirmPerson", personId: "alice" }, personName: "Alice" }] },
    remoteFacts: { facts: [{ id: "fact", revision: "remote", region: { x: 0.1, y: 0.2, width: 0.2, height: 0.2 }, decision: { decision: "notFace" } }] },
  }],
}));

it("shows both human decisions before resolving and requires confirmation to clear", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => root.render(<QueryClientProvider client={client}><FaceSyncPanel t={(key) => key} /></QueryClientProvider>));
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
    expect(container.textContent).toContain("Alice peopleState_confirmed");
    expect(container.textContent).toContain("peopleState_notFace");
    const button = (text: string) => Array.from(container.querySelectorAll("button")).find((item) => item.textContent === text)!;
    await act(async () => button("faceSyncUseSidecar").click());
    expect(api.resolve).toHaveBeenCalledWith("/photos/portrait.jpg", true);
    await act(async () => button("faceClearCache").click());
    expect(api.clear).not.toHaveBeenCalled();
    confirm.mockReturnValue(true);
    await act(async () => button("faceClearCache").click());
    expect(api.clear).toHaveBeenCalledOnce();
  } finally {
    await act(async () => root.unmount());
    client.clear();
    confirm.mockRestore();
    vi.unstubAllGlobals();
  }
});
