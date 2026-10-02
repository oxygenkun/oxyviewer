// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PersonClusters } from "./PersonClusters";
import type { PersonClusterSnapshot } from "@/types";
const api = vi.hoisted(() => ({ adopt: vi.fn() }));
vi.mock("@/lib/api", () => ({ adoptPersonCluster: api.adopt }));
vi.mock("@/components/browsing/Thumbnail", () => ({ Thumbnail: () => null }));
let host: HTMLDivElement; let root: Root; let client: QueryClient;
const select = vi.fn(); const adopted = vi.fn();
const snapshot: PersonClusterSnapshot = { snapshotId: "s1", runId: "r1", folderPath: "C:/photos", algorithm: "test", pipelineFingerprint: "fp", clusters: [{ id: "c1", cover: null, people: [], members: [{ assetPath: "C:/photos/a.jpg", instanceId: "face", sourceRevision: "v1", summaryRevision: "1:1", faceBox: [0,0,1,1] }] }], ungrouped: [], noFaceCount: 0, unavailableCount: 0 };
const person = { id: "p1", folderPath: "C:/photos", displayName: "已有姓名", identityConfirmed: true, revision: 1, referenceInstanceId: "ref", pendingCount: 0 };
const button = (text: string) => [...host.querySelectorAll("button")].find(b => b.textContent === text)!;
async function render(selected?: { snapshotId: string; clusterId: string }) {
  await act(async () => root.render(<QueryClientProvider client={client}><PersonClusters folderPath="C:/photos" snapshot={snapshot} people={[person]} selected={selected} onSelect={select} onAdopted={adopted} /></QueryClientProvider>));
}
beforeEach(() => { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host=document.createElement("div"); document.body.append(host); root=createRoot(host); client=new QueryClient(); });
afterEach(async () => { await act(async () => root.unmount()); client.clear(); host.remove(); vi.clearAllMocks(); vi.unstubAllGlobals(); });
it("browses anonymous groups without creating user facts, and clears the filter", async () => {
  await render();
  await act(async () => button("匿名组 1匿名建议1 张照片").click());
  expect(select).toHaveBeenCalledWith({ snapshotId: "s1", clusterId: "c1" });
  expect(api.adopt).not.toHaveBeenCalled();
  await render({ snapshotId: "s1", clusterId: "c1" });
  await act(async () => button("清除分组筛选").click());
  expect(select).toHaveBeenLastCalledWith(undefined);
});
it("adds to an existing named person only through an explicit pending-review action", async () => {
  api.adopt.mockResolvedValue({ person, added: 1, preserved: 2, conflicted: 0 });
  await render({ snapshotId: "s1", clusterId: "c1" });
  const field = host.querySelector("select")!;
  await act(async () => { field.value="p1"; field.dispatchEvent(new Event("change", { bubbles: true })); });
  await act(async () => button("加入待确认").click());
  expect(api.adopt).toHaveBeenCalledExactlyOnceWith("C:/photos", "s1", "c1", "p1", undefined);
  expect(adopted).toHaveBeenCalledWith("C:/photos", person);
  expect(host.textContent).toContain("保留原决定 2 个");
  await render({ snapshotId: "old", clusterId: "c1" });
  expect(host.querySelector("form")).toBeNull();
  expect(host.textContent).toContain("分组已更新");
});
