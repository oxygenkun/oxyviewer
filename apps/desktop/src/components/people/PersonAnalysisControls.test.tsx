// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PersonAnalysisControls } from "./PersonAnalysisControls";
import type { PersonModelStatus, PersonOperationStatus } from "@/types";

const api = vi.hoisted(() => ({ download: vi.fn(), cancel: vi.fn(), models: vi.fn(), operation: vi.fn() }));
vi.mock("@/lib/api", () => ({ cancelPersonOperation: api.cancel,
  getPersonModels: api.models, getPersonOperation: api.operation, downloadPersonModel: api.download, importPersonModel: vi.fn() }));
let host: HTMLDivElement;
let root: Root;
let client: QueryClient;
const ready: PersonModelStatus[] = [{ id: "face", name: "face", installed: true, downloadAvailable: true, sizeBytes: 1, sourceUrl: "", usage: "", error: null }];
const button = (label: string) => [...host.querySelectorAll("button")].find(item => item.textContent === label)!;
async function render() {
  await act(async () => root.render(<QueryClientProvider client={client}><PersonAnalysisControls /></QueryClientProvider>));
}
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  api.models.mockResolvedValue(ready); api.operation.mockResolvedValue(null);
  api.download.mockResolvedValue(undefined); api.cancel.mockResolvedValue(undefined);
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["person-models"], ready);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); client.clear(); host.remove(); vi.clearAllMocks(); vi.unstubAllGlobals(); });

it("downloads the selected missing model from model management", async () => {
  const missing = [{ ...ready[0], installed: false }];
  api.models.mockResolvedValue(missing); client.setQueryData(["person-models"], missing);
  await render();
  await act(async () => button("管理模型").click());
  await act(async () => button("下载").click());
  expect(api.download).toHaveBeenCalledExactlyOnceWith("face");
});

it("keeps a running task scoped to its original folder and cancels by operation id", async () => {
  const pending: PersonOperationStatus = { operationId: "old-folder-job", folderPath: "C:/first", state: "analysing", completed: 1, total: 4, detail: "正在识别", error: null, run: null };
  api.operation.mockResolvedValue(pending); client.setQueryData(["person-operation"], pending);
  await render();
  expect(host.textContent).toContain("任务文件夹：first");
  await act(async () => button("取消任务").click());
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith("old-folder-job");
  expect(button("正在取消…").disabled).toBe(true);
});

it("blocks model installation while a grouping job still owns the operation", async () => {
  client.setQueryData(["person-models"], [{ ...ready[0], installed: false }]);
  api.models.mockResolvedValue([{ ...ready[0], installed: false }]);
  const pending = { operationId: "cluster", folderPath: "C:/saved", state: "clustering", completed: 0, total: 0, detail: "聚类中", error: null, run: null };
  api.operation.mockResolvedValue(pending);
  await act(async () => { client.setQueryData(["person-operation"], pending); });
  await render();
  await act(async () => button("管理模型").click());
  expect(button("下载").disabled).toBe(true);
  expect(api.download).not.toHaveBeenCalled();
});
