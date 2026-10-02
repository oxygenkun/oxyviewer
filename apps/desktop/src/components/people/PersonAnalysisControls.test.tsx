// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PersonAnalysisControls } from "./PersonAnalysisControls";
import type { PersonModelStatus, PersonOperationStatus } from "@/types";

const api = vi.hoisted(() => ({ cluster: vi.fn(), start: vi.fn(), cancel: vi.fn(), models: vi.fn(), operation: vi.fn() }));
vi.mock("@/lib/api", () => ({ startPersonClustering: api.cluster, startFolderPersonAnalysis: api.start, cancelPersonOperation: api.cancel,
  getPersonModels: api.models, getPersonOperation: api.operation, downloadPersonModel: vi.fn(), importPersonModel: vi.fn() }));
let host: HTMLDivElement;
let root: Root;
let client: QueryClient;
const ready: PersonModelStatus[] = [{ id: "face", name: "face", installed: true, downloadAvailable: true, sizeBytes: 1, sourceUrl: "", usage: "", error: null }];
const button = (label: string) => [...host.querySelectorAll("button")].find(item => item.textContent === label)!;
async function render(folder: string) {
  await act(async () => root.render(<QueryClientProvider client={client}><PersonAnalysisControls sessionId="session" folderPath={folder} /></QueryClientProvider>));
}
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  api.models.mockResolvedValue(ready); api.operation.mockResolvedValue(null);
  api.start.mockResolvedValue(undefined); api.cancel.mockResolvedValue(undefined);
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["person-models"], ready);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); client.clear(); host.remove(); vi.clearAllMocks(); vi.unstubAllGlobals(); });

it("uses the newly selected folder when starting recognition", async () => {
  await render("C:/first"); await render("C:/second");
  await act(async () => button("识别选中文件夹").click());
  expect(api.start).toHaveBeenCalledExactlyOnceWith("session", "C:/second");
});

it("keeps a running task scoped to its original folder and cancels by operation id", async () => {
  const pending: PersonOperationStatus = { operationId: "old-folder-job", folderPath: "C:/first", state: "analysing", completed: 1, total: 4, detail: "正在识别", error: null, run: null };
  api.operation.mockResolvedValue(pending); client.setQueryData(["person-operation"], pending);
  await render("C:/second");
  expect(button("识别选中文件夹").disabled).toBe(true);
  expect(host.textContent).toContain("任务文件夹：first");
  await act(async () => button("取消任务").click());
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith("old-folder-job");
  expect(button("正在取消…").disabled).toBe(true);
});

it("allows clustering saved features without installed models and blocks a second job", async () => {
  client.setQueryData(["person-models"], [{ ...ready[0], installed: false }]);
  api.models.mockResolvedValue([{ ...ready[0], installed: false }]);
  await render("C:/saved");
  expect(button("识别选中文件夹").disabled).toBe(true);
  expect(button("聚类已有结果").disabled).toBe(false);
  await act(async () => button("聚类已有结果").click());
  expect(api.cluster).toHaveBeenCalledExactlyOnceWith("session", "C:/saved");
  const pending = { operationId: "cluster", folderPath: "C:/saved", state: "clustering", completed: 0, total: 0, detail: "聚类中", error: null, run: null };
  api.operation.mockResolvedValue(pending);
  await act(async () => { client.setQueryData(["person-operation"], pending); });
  await render("C:/another");
  expect(button("聚类已有结果").disabled).toBe(true);
});
