// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PersonAnalysisControls } from "./PersonAnalysisControls";
import type { PersonModelStatus, PersonOperationStatus } from "@/types";

const api = vi.hoisted(() => ({ start: vi.fn(), cancel: vi.fn(), models: vi.fn(), operation: vi.fn() }));
vi.mock("@/lib/api", () => ({ startFolderPersonAnalysis: api.start, cancelPersonOperation: api.cancel,
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
  expect(host.textContent).toContain("任务文件夹：C:/first");
  await act(async () => button("取消任务").click());
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith("old-folder-job");
  expect(button("正在取消…").disabled).toBe(true);
});
