// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GlobalPeopleContext, useGlobalPeople } from "./GlobalPeopleContext";
import { PeopleLibrary } from "./PeopleLibrary";
import type { PersonTuple, PersonTupleFilter } from "@/types";

const api = vi.hoisted(() => ({ folders: vi.fn(), openFolder: vi.fn(), gallery: vi.fn(), catalog: vi.fn(), asset: vi.fn() }));
vi.mock("@/lib/api", () => ({ getGlobalPersonFolders: api.folders, getGlobalPersonGallery: api.gallery, listGlobalPeople: api.catalog, getPersonTupleAsset: api.asset, getFolderPeopleWorkspace: vi.fn() }));
vi.mock("../browsing/Thumbnail", () => ({ Thumbnail: ({ asset, crossFolder }: { asset: { path: string }; crossFolder?: boolean }) => <span data-preview-path={asset.path} data-cross-folder={crossFolder}>照片预览</span> }));
const tuple: PersonTuple = { id: "t", assetPath: "C:/earlier-shoot/portrait.jpg", sourceRevision: "1:1", sourceIdentityRevision: "v1", faceBox: [.1, .1, .2, .2], bodyBox: null, revision: 1, needsReview: false, personId: "p", decision: "belongs", score: null };
let host: HTMLDivElement, root: Root, client: QueryClient;
function Harness() {
  const [filter, setFilter] = useState<PersonTupleFilter>();
  const state = useGlobalPeople(undefined, true, filter, setFilter);
  return <GlobalPeopleContext.Provider value={state}><PeopleLibrary onOpenFolder={api.openFolder} /></GlobalPeopleContext.Provider>;
}
async function settle() { await act(async () => { await new Promise(resolve => setTimeout(resolve, 15)); }); }
const click = async (name: string) => { await act(async () => [...host.querySelectorAll('button')].find(b => b.textContent === name || b.getAttribute('aria-label') === name)!.click()); await settle(); };
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("IntersectionObserver", class { constructor(private callback: (entries: { isIntersecting: boolean }[]) => void) {} observe() { this.callback([{ isIntersecting: true }]); } disconnect() {} });
  api.catalog.mockResolvedValue([{ id: "p", displayName: "林间", revision: 1, referenceInstanceIds: [], tagId: null }]);
  api.gallery.mockResolvedValue({ tuples: [tuple], total: 1, folderCount: 1 });
  api.folders.mockResolvedValue({ folders: [{ folderPath: "C:/earlier-shoot", coverAssetPath: tuple.assetPath, photoCount: 31, instanceCount: 32 }], total: 1 });
  api.openFolder.mockResolvedValue(undefined);
  api.asset.mockResolvedValue({ id: "photo", path: tuple.assetPath, sizeBytes: 1, modifiedAtMs: 1 });
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
  await act(async () => root.render(<QueryClientProvider client={client}><Harness /></QueryClientProvider>)); await settle(); await settle();
});
afterEach(async () => { await act(async () => root.unmount()); client.clear(); host.remove(); vi.clearAllMocks(); vi.unstubAllGlobals(); });

it("organizes confirmed history by folder and opens only on double click", async () => {
  await click("浏览 林间");
  expect(api.folders).toHaveBeenCalledWith("p", 0, 24);
  expect(host.textContent).toContain("31 张已确认照片 · 32 个标记");
  await settle();
  const card = host.querySelector('[aria-label="打开文件夹 C:/earlier-shoot"]')!;
  expect(card.querySelector('[data-preview-path]')?.getAttribute('data-preview-path')).toBe(tuple.assetPath);
  expect(card.querySelector('[data-cross-folder]')?.getAttribute('data-cross-folder')).toBe("true");
  expect(card.querySelector('small')?.title).toBe("C:/earlier-shoot");
  await click("打开文件夹 C:/earlier-shoot");
  expect(api.openFolder).not.toHaveBeenCalled();
  await act(async () => host.querySelector('[aria-label="打开文件夹 C:/earlier-shoot"]')!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
  expect(api.openFolder).toHaveBeenCalledWith("C:/earlier-shoot");
});

it("keeps unavailable folders visible and reports failed opens in place", async () => {
  api.openFolder.mockRejectedValue(new Error("offline"));
  api.asset.mockRejectedValue(new Error("offline"));
  await click("浏览 林间");
  await settle();
  const card = host.querySelector('[aria-label="打开文件夹 C:/earlier-shoot"]')!;
  expect(card.textContent).toContain("原图暂不可用");
  await act(async () => card.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })));
  await settle();
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("offline");
  expect(host.querySelector('[aria-label="人物文件夹"]')).not.toBeNull();
  expect(card.hasAttribute("disabled")).toBe(false);
});

it("pages complete folders rather than grouping one page of photos", async () => {
  api.folders.mockResolvedValue({ folders: [{ folderPath: "C:/earlier-shoot", coverAssetPath: tuple.assetPath, photoCount: 100, instanceCount: 100 }], total: 25 });
  await click("浏览 林间"); await click("下一页");
  expect(api.folders).toHaveBeenLastCalledWith("p", 24, 24);
  expect(api.gallery).not.toHaveBeenCalledWith("p", 0, 24);
});

it("defers folder cover lookup until the card enters the viewport", async () => {
  let reveal: (() => void) | undefined;
  vi.stubGlobal("IntersectionObserver", class {
    constructor(callback: (entries: { isIntersecting: boolean }[]) => void) { reveal = () => callback([{ isIntersecting: true }]); }
    observe() {} disconnect() {}
  });
  api.asset.mockClear();
  await click("浏览 林间");
  expect(api.asset).not.toHaveBeenCalled();
  await act(async () => reveal!()); await settle();
  expect(api.asset).toHaveBeenCalledWith(tuple.assetPath);
});
