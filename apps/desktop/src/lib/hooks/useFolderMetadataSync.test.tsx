// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AssetSummary, EmbeddedSyncProgress, EmbeddedSyncResult, FolderSession } from "@/types";
import { useFolderMetadataSync } from "./useFolderMetadataSync";

const api = vi.hoisted(() => ({
  listAssets: vi.fn(), syncEmbeddedMetadataBatch: vi.fn(), onEmbeddedSyncProgress: vi.fn(),
  cancelMetadataSync: vi.fn(), getExiftoolStatus: vi.fn(), installExiftool: vi.fn(), chooseAndConfigureExiftool: vi.fn(),
}));
vi.mock("@/lib/api", () => api);
const session: FolderSession = { id: "root-session", rootPath: "/photos", displayName: "Photos", openedAtMs: 1, deletionMode: "trash" };
const directory = "/photos/other-folder";
function asset(name: string, kind: AssetSummary["kind"] = "jpeg", hasSidecar = true): AssetSummary {
  return { id: name, path: `${directory}/${name}`, name, kind, hasSidecar, extension: name.split(".").at(-1)!, sizeBytes: 10, modifiedAtMs: 1 };
}
function page(items: AssetSummary[], nextCursor: number | null = null) { return { items, nextCursor, total: items.length, snapshotRevision: 7 }; }
function reply(values: Partial<EmbeddedSyncResult> = {}): EmbeddedSyncResult {
  return { jobId: "job", total: 1, completed: 1, updated: 1, unchanged: 0, skipped: 0, cancelled: false, failures: [], ...values };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
let host: HTMLDivElement, root: Root, client: QueryClient;
let sync: ReturnType<typeof useFolderMetadataSync>;
let progress: (value: EmbeddedSyncProgress) => void;
const unlisten = vi.fn();
function Harness() { sync = useFolderMetadataSync(key => key); return null; }
function emit(values: Partial<EmbeddedSyncProgress> = {}) {
  const [paths, requestId] = api.syncEmbeddedMetadataBatch.mock.calls.at(-1)!;
  progress({ requestId, jobId: "job", total: paths.length, completed: 0, updated: 0, unchanged: 0, skipped: 0, ...values });
}
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.resetAllMocks();
  api.getExiftoolStatus.mockResolvedValue({ available: true });
  api.cancelMetadataSync.mockResolvedValue(true);
  api.onEmbeddedSyncProgress.mockImplementation(async callback => { progress = callback; return unlisten; });
  api.syncEmbeddedMetadataBatch.mockImplementation(async paths => reply({ total: paths.length, completed: paths.length, updated: paths.length }));
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.spyOn(client, "invalidateQueries");
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  await act(async () => root.render(<StrictMode><QueryClientProvider client={client}><Harness /></QueryClientProvider></StrictMode>));
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); client.clear(); vi.unstubAllGlobals(); });

it("collects every page without filters and submits exactly one native batch after subscribing", async () => {
  const a = asset("a.jpg"), b = asset("b.hif", "heif");
  api.listAssets.mockResolvedValueOnce(page([a, asset("a.arw", "raw")], 2))
    .mockResolvedValueOnce(page([b, a, asset("none.jpg", "jpeg", false), asset("other.png", "png")]));
  await act(async () => sync.start(session, directory));
  expect(api.listAssets.mock.calls).toEqual([
    [session.id, directory, { sort: "name", direction: "ascending", pageSize: 250 }, 0, undefined],
    [session.id, directory, { sort: "name", direction: "ascending", pageSize: 250 }, 2, 7],
  ]);
  expect(api.syncEmbeddedMetadataBatch).toHaveBeenCalledExactlyOnceWith([a.path, b.path], expect.any(String));
  expect(api.onEmbeddedSyncProgress.mock.invocationCallOrder[0]).toBeLessThan(api.syncEmbeddedMetadataBatch.mock.invocationCallOrder[0]);
  expect(sync.state).toMatchObject({ phase: "done", total: 2, completed: 2, succeeded: 2, unchanged: 0, skipped: 3 });
  expect(client.invalidateQueries).toHaveBeenCalledTimes(4);
  expect(unlisten).toHaveBeenCalledOnce();
});

it("keeps the target through manual provider rechecks and never downloads implicitly", async () => {
  const a = asset("a.jpg"); api.listAssets.mockResolvedValue(page([a]));
  api.getExiftoolStatus.mockResolvedValue({ available: false });
  await act(async () => sync.start(session, directory));
  await act(async () => sync.setup("check"));
  expect(sync.state).toMatchObject({ phase: "setup", error: "exiftoolNotFound", settingUp: false });
  expect(api.syncEmbeddedMetadataBatch).not.toHaveBeenCalled();
  await act(async () => sync.start(session, "/photos/different"));
  expect(api.listAssets).toHaveBeenCalledTimes(1);
  api.getExiftoolStatus.mockResolvedValue({ available: true });
  await act(async () => sync.setup("check"));
  expect(api.syncEmbeddedMetadataBatch).toHaveBeenCalledExactlyOnceWith([a.path], expect.any(String));
  expect(api.installExiftool).not.toHaveBeenCalled(); expect(api.chooseAndConfigureExiftool).not.toHaveBeenCalled();
});

it("uses native counts and per-file failures, including uncertain writes and newly skipped files", async () => {
  api.listAssets.mockResolvedValue(page([asset("a.jpg"), asset("b.jpg"), asset("c.jpg"), asset("d.jpg")]));
  const failures = [{ path: asset("b.jpg").path, error: "lost confirmation", uncertain: true }];
  api.syncEmbeddedMetadataBatch.mockResolvedValue(reply({ total: 4, completed: 4, updated: 1, unchanged: 1, skipped: 1, failures }));
  await act(async () => sync.start(session, directory));
  expect(sync.state).toMatchObject({ phase: "done", completed: 4, succeeded: 1, unchanged: 1, failureCount: 1, skipped: 1, failures });
  expect(api.syncEmbeddedMetadataBatch).toHaveBeenCalledOnce();
  expect(client.invalidateQueries).toHaveBeenCalledTimes(4);
});

it("does not refresh browsing or details when all files are unchanged", async () => {
  api.listAssets.mockResolvedValue(page([asset("a.jpg")]));
  api.syncEmbeddedMetadataBatch.mockResolvedValue(reply({ updated: 0, unchanged: 1 }));
  await act(async () => sync.start(session, directory));
  expect(sync.state).toMatchObject({ phase: "done", succeeded: 0, unchanged: 1 });
  expect(client.invalidateQueries).not.toHaveBeenCalled();
});

it("routes cancellation to the job and retains the final partial result without retrying", async () => {
  const pending = deferred<EmbeddedSyncResult>();
  api.listAssets.mockResolvedValue(page([asset("a.jpg"), asset("b.jpg")]));
  api.syncEmbeddedMetadataBatch.mockReturnValue(pending.promise);
  let writing!: Promise<void>;
  await act(async () => { writing = sync.start(session, directory); });
  await act(async () => { emit({ completed: 1, updated: 1 }); sync.cancel(); });
  expect(api.cancelMetadataSync).toHaveBeenCalledWith("job");
  await act(async () => { pending.resolve(reply({ total: 2, completed: 1, cancelled: true })); await writing; });
  expect(sync.state).toMatchObject({ phase: "done", succeeded: 1, completed: 1, cancelled: true });
  expect(api.syncEmbeddedMetadataBatch).toHaveBeenCalledOnce();
  expect(client.invalidateQueries).toHaveBeenCalledTimes(4);
});

it("handles cancellation before the initial progress supplies a job ID", async () => {
  const pending = deferred<EmbeddedSyncResult>();
  api.listAssets.mockResolvedValue(page([asset("a.jpg")])); api.syncEmbeddedMetadataBatch.mockReturnValue(pending.promise);
  let writing!: Promise<void>;
  await act(async () => { writing = sync.start(session, directory); });
  await act(async () => sync.cancel());
  expect(api.cancelMetadataSync).not.toHaveBeenCalled();
  await act(async () => emit());
  expect(api.cancelMetadataSync).toHaveBeenCalledWith("job");
  await act(async () => { pending.resolve(reply({ completed: 0, updated: 0, cancelled: true })); await writing; });
  expect(sync.state).toMatchObject({ phase: "done", cancelled: true, completed: 0 });
});

it("rejects stale pages before native admission", async () => {
  api.listAssets.mockResolvedValueOnce(page([asset("a.jpg")], 1)).mockResolvedValueOnce({ ...page([]), progress: { stage: "stale" } });
  await act(async () => sync.start(session, directory));
  expect(sync.state?.error).toContain("folderMetadataSyncStale");
  expect(api.syncEmbeddedMetadataBatch).not.toHaveBeenCalled(); expect(api.getExiftoolStatus).not.toHaveBeenCalled();
});

it("keeps an interrupted or lost reply visible and refreshes possible writes without replaying", async () => {
  api.listAssets.mockResolvedValue(page([asset("a.jpg")]));
  api.syncEmbeddedMetadataBatch.mockRejectedValue(new Error("reply lost"));
  await act(async () => sync.start(session, directory));
  expect(sync.state).toMatchObject({ phase: "done", error: "Error: reply lost" });
  expect(api.syncEmbeddedMetadataBatch).toHaveBeenCalledOnce(); expect(client.invalidateQueries).toHaveBeenCalledTimes(4);
});

it("ignores late progress after the authoritative result and foreign request IDs", async () => {
  api.listAssets.mockResolvedValue(page([asset("a.jpg")]));
  await act(async () => sync.start(session, directory));
  await act(async () => { emit({ requestId: "other", completed: 99 }); emit({ completed: 0, updated: 0 }); });
  expect(sync.state).toMatchObject({ phase: "done", completed: 1, succeeded: 1 });
});

it("cancels an unmounted batch when its initial job ID arrives", async () => {
  const pending = deferred<EmbeddedSyncResult>();
  api.listAssets.mockResolvedValue(page([asset("a.jpg")])); api.syncEmbeddedMetadataBatch.mockReturnValue(pending.promise);
  let writing!: Promise<void>;
  await act(async () => { writing = sync.start(session, directory); });
  await act(async () => root.render(null));
  await act(async () => emit());
  expect(api.cancelMetadataSync).toHaveBeenCalledWith("job");
  await act(async () => { pending.resolve(reply({ completed: 0, updated: 0, cancelled: true })); await writing; });
  expect(unlisten).toHaveBeenCalled();
});
