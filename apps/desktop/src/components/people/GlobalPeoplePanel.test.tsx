// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { GlobalPeopleContext, useGlobalPeople } from "./GlobalPeopleContext";
import { GlobalPeoplePanel } from "./GlobalPeoplePanel";
import { GlobalPersonBoxes } from "./GlobalPersonBoxes";
import type { FolderPeopleWorkspace, PersonTuple, PersonTupleFilter, AssetSummary, GlobalPerson } from "@/types";
const api = vi.hoisted(() => ({ review: vi.fn(), start: vi.fn(), asset: vi.fn(), catalog: vi.fn(), workspace: vi.fn(), operation: vi.fn(), refs: vi.fn(), tags: vi.fn(), geometry: vi.fn() }));
vi.mock("@/lib/api", () => ({ reviewPersonTuples: api.review, startPeopleGrouping: api.start, getPersonTupleAsset: api.asset, listGlobalPeople: api.catalog, getFolderPeopleWorkspace: api.workspace, getPersonOperation: api.operation, getGlobalPersonReferences: api.refs, listCustomTags: api.tags, saveGlobalPerson: vi.fn(), setGlobalPersonReference: vi.fn(), setGlobalPersonTag: vi.fn(), savePersonTupleGeometry: api.geometry }));
vi.mock("./PersonAnalysisControls", () => ({ PersonAnalysisControls: () => null }));
vi.mock("../browsing/Thumbnail", () => ({ Thumbnail: () => null }));
const person: GlobalPerson = { id: "p", displayName: "共享人物", revision: 1, referenceInstanceIds: ["reference"], tagId: null };
const tuple = (id: string): PersonTuple => ({ id, assetPath: "C:/photos/two.jpg", sourceRevision: "1:1", sourceIdentityRevision: "source", faceBox: [.1, .1, .2, .2], bodyBox: id === "one" ? [.1, .1, .3, .8] : null, revision: 0, needsReview: false, personId: null, decision: null, score: null });
const data: FolderPeopleWorkspace = { folderPath: "C:/photos", revision: "revision", groups: [{ id: "anonymous", personId: null, members: [tuple("one"), tuple("two")], cover: null }], unknownCount: 1, knownCount: 0, noFaceCount: 0, unavailableCount: 0, hasAnalysis: true, notice: null };
const asset = { id: "photo", path: "C:/photos/two.jpg", sizeBytes: 1, modifiedAtMs: 1 } as AssetSummary;
let host: HTMLDivElement, root: Root, client: QueryClient;
function Harness({ selectedAssetPaths }: { selectedAssetPaths?: string[] }) { const [filter, setFilter] = useState<PersonTupleFilter | undefined>({ groupId: "anonymous" }); const state = useGlobalPeople("C:/photos", true, filter, setFilter); return <GlobalPeopleContext.Provider value={state}><GlobalPeoplePanel sessionId="session" activeAsset={asset} selectedAssetPaths={selectedAssetPaths} /><GlobalPersonBoxes asset={asset} /></GlobalPeopleContext.Provider>; }
const button = (text: string) => [...host.querySelectorAll("button")].find(b => b.textContent === text)!;
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); api.catalog.mockResolvedValue([person]); api.workspace.mockResolvedValue(data); api.operation.mockResolvedValue(null); api.refs.mockResolvedValue([]); api.tags.mockResolvedValue([]); api.asset.mockResolvedValue(asset); api.review.mockResolvedValue(1); api.start.mockResolvedValue(undefined);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host); client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } }); client.setQueryData(["global-people"], [person]); client.setQueryData(["people-workspace", "C:/photos"], data);
  await act(async () => root.render(<QueryClientProvider client={client}><Harness /></QueryClientProvider>));
});
afterEach(async () => { await act(async () => root.unmount()); client.clear(); host.remove(); vi.clearAllMocks(); vi.unstubAllGlobals(); });
async function target() { const field = host.querySelector<HTMLSelectElement>('[aria-label="实例指定人物"]')!; await act(async () => { field.value = "p"; field.dispatchEvent(new Event("change", { bubbles: true })); }); }
it("transfers all tuples, including distinct people in the same photograph, only on explicit action", async () => {
  await target(); expect(api.review).not.toHaveBeenCalled(); await act(async () => button("整组转为待确认").click());
  expect(api.review).toHaveBeenCalledWith(data, data.groups[0].members, "p", "pending", false);
});
it("assigns only checked tuples and exposes the same tuple through face and body boxes", async () => {
  await target(); await act(async () => host.querySelector<HTMLInputElement>('[aria-label="选择实例 2"]')!.click());
  await act(async () => button("指定为待确认").click()); expect(api.review.mock.calls[0][1].map((t: PersonTuple) => t.id)).toEqual(["two"]);
  await act(async () => button("显示全部照片").click());
  const face = host.querySelector<HTMLButtonElement>('[aria-label="选择实例 1 面部"]')!;
  await act(async () => face.click());
  expect(host.querySelector('[aria-label="选择实例 1 人体"]')?.classList.contains("is-selected")).toBe(true);
  expect(host.querySelector('[aria-label="选择实例 2 面部"]')?.classList.contains("is-selected")).toBe(false);
});
it("keeps automatic grouping and selected identity search as separate requests", async () => {
  await act(async () => button("自动分组").click()); expect(api.start).toHaveBeenLastCalledWith("session", "C:/photos", null, .4, false);
  await act(async () => button("寻找人物").click());
  const label = [...host.querySelectorAll("label")].find(l => l.textContent?.includes("共享人物 · 1 个参考"))!;
  await act(async () => label.querySelector<HTMLInputElement>("input")!.click()); await act(async () => button("开始寻找").click());
  expect(api.start).toHaveBeenLastCalledWith("session", "C:/photos", ["p"], .4, false);
});

it("opens a newly drawn body-only instance for review without assigning the other people", async () => {
  const created:PersonTuple={...tuple("new"),faceBox:null,bodyBox:[.1,.1,.5,.8],revision:1};
  api.geometry.mockImplementation(async()=>{
    const next={...data,groups:[...data.groups,{id:"manual:new",personId:null,members:[created],cover:null}],unknownCount:2};
    api.workspace.mockResolvedValue(next);client.setQueryData(["people-workspace","C:/photos"],next);return created;
  });
  await act(async()=>button("＋ 人体实例").click());
  const canvas=host.querySelector<HTMLDivElement>('[aria-label="人物实例画布"]')!;
  canvas.setPointerCapture=vi.fn();
  vi.spyOn(canvas,"getBoundingClientRect").mockReturnValue({left:0,top:0,width:100,height:100} as DOMRect);
  await act(async()=>canvas.dispatchEvent(new MouseEvent("pointerdown",{bubbles:true,button:0,clientX:10,clientY:10})));
  await act(async()=>canvas.dispatchEvent(new MouseEvent("pointerup",{bubbles:true,button:0,clientX:60,clientY:90})));
  expect(api.geometry).toHaveBeenCalledWith("C:/photos",asset,null,[.1,.1,.5,.8],undefined);
  expect(host.querySelector('[aria-label="选择实例 3 人体"]')?.classList.contains("is-selected")).toBe(true);
  expect(host.querySelector('[aria-label="选择实例 3 面部"]')).toBeNull();
  expect(api.review).not.toHaveBeenCalled();
});

it("keeps an anonymous instance visible in its named group after direct confirmation", async () => {
  await target();
  api.review.mockImplementation(async()=>{
    const confirmed={...tuple("two"),personId:"p",decision:"belongs" as const,revision:1};
    const next={...data,groups:[{...data.groups[0],members:[tuple("one")]},{id:"person:p",personId:"p",members:[confirmed],cover:null}],knownCount:1};
    api.workspace.mockResolvedValue(next);client.setQueryData(["people-workspace","C:/photos"],next);return 1;
  });
  await act(async()=>host.querySelector<HTMLInputElement>('[aria-label="选择实例 2"]')!.click());
  await act(async()=>button("已确认").click());
  expect(host.querySelector('[aria-label="实例归属审阅"]')?.textContent).toContain("共享人物 · 实例归属");
  expect(host.querySelector<HTMLSelectElement>('[aria-label="实例审阅过滤"]')?.value).toBe("all");
  expect(host.querySelector('[aria-label="选择实例 1 面部"]')).not.toBeNull();
  expect(host.querySelector('[aria-label="选择实例 2 面部"]')).not.toBeNull();
});

it("confirms one person directly beside the current photo without selecting the other person", async () => {
  await target();
  const quick = host.querySelector('[aria-label="看图确认"]')!;
  const confirm = [...quick.querySelectorAll('button')].filter(b => b.textContent === '确认为 共享人物');
  expect(confirm).toHaveLength(2);
  await act(async () => confirm[1].click());
  expect(api.review).toHaveBeenCalledWith(data, [data.groups[0].members[1]], 'p', 'belongs', false);
});

it("shows the global library navigation and hides folder review while browsing the library", async () => {
  const navigation = host.querySelector('[aria-label="人物工作区导航"]')!;
  await act(async () => navigation.querySelector<HTMLButtonElement>('button')!.click());
  expect(host.querySelector('[aria-label="看图确认"]')).toBeNull();
  expect(host.querySelector('[aria-label="全局人物姓名"]')).not.toBeNull();
  expect(navigation.querySelector('button')?.getAttribute('aria-pressed')).toBe('true');
});


it("uses each person's own candidate identity for one-click review in a multi-person photo", async () => {
  const second = { ...person, id: 'second', displayName: '另一位' };
  const a = { ...tuple('one'), personId: person.id, decision: 'pending' as const };
  const b = { ...tuple('two'), personId: second.id, decision: 'pending' as const };
  const next = { ...data, groups: [{id:'person:p',personId:'p',members:[a],cover:null}, {id:'person:second',personId:'second',members:[b],cover:null}] };
  await act(async () => { client.setQueryData(['global-people'], [person, second]); client.setQueryData(['people-workspace', 'C:/photos'], next); });
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)); });
  await act(async () => button('确认为 另一位').click());
  expect(api.review.mock.calls[0][1]).toEqual([b]);
  expect(api.review.mock.calls[0][2]).toBe('second');
});

async function gridSelection(paths: string[]) {
  await act(async () => root.render(<QueryClientProvider client={client}><Harness selectedAssetPaths={paths} /></QueryClientProvider>));
}

it.each([['批量确认为 共享人物', 'belongs'], ['批量不属于', 'doesNotBelong'], ['批量暂缓', 'deferred']] as const)("%s reviews only selected photos within the current group", async (label, decision) => {
  const a = { ...tuple('one'), personId: 'p', decision: 'pending' as const };
  const b = { ...tuple('three'), assetPath: 'C:/photos/three.jpg', personId: 'p', decision: 'pending' as const };
  const outside = { ...b, id: 'outside', assetPath: 'C:/photos/outside.jpg' };
  const otherPerson = { ...tuple('two'), personId: 'other', decision: 'pending' as const };
  const next = { ...data, groups: [{ ...data.groups[0], personId: 'p', members: [a, b, outside] }, { id: 'other', personId: 'other', members: [otherPerson], cover: null }] };
  api.workspace.mockResolvedValue(next);
  await act(async () => client.setQueryData(['people-workspace', 'C:/photos'], next));
  await gridSelection([a.assetPath, b.assetPath]);
  const quick = host.querySelector('[aria-label="看图确认"]')!;
  expect(quick.textContent).toContain('已选 2 张 · 2 个实例');
  expect(quick.querySelector('[aria-label="共享人物已选实例数"]')?.textContent).toBe('2');
  await act(async () => button(label).click());
  expect(api.review).toHaveBeenCalledWith(next, [a, b], 'p', decision, false);
  expect(api.review).toHaveBeenCalledTimes(1);
});

it("updates batch counts on deselection and restores individual review for a single photo", async () => {
  const b = { ...tuple('three'), assetPath: 'C:/photos/three.jpg' };
  const c = { ...tuple('four'), assetPath: 'C:/photos/four.jpg' };
  const next = { ...data, groups: [{ ...data.groups[0], members: [...data.groups[0].members, b, c] }] };
  await act(async () => client.setQueryData(['people-workspace', 'C:/photos'], next));
  await gridSelection([asset.path, b.assetPath, c.assetPath]);
  expect(host.querySelector('[aria-label="未知人物已选实例数"]')?.textContent).toBe('4');
  await gridSelection([asset.path, b.assetPath]);
  expect(host.querySelector('[aria-label="未知人物已选实例数"]')?.textContent).toBe('3');
  await gridSelection([asset.path]);
  expect(host.querySelector('[aria-label="看图确认"]')?.textContent).toContain('2 人');
  expect(host.querySelector('[aria-label="未知人物已选实例数"]')).toBeNull();
  expect(api.review).not.toHaveBeenCalled();
});

it("honors the decision filter and blocks batch confirmation when geometry needs review", async () => {
  const a = { ...tuple('one'), personId: 'p', decision: 'pending' as const, needsReview: true };
  const b = { ...tuple('three'), assetPath: 'C:/photos/three.jpg', personId: 'p', decision: 'belongs' as const };
  const next = { ...data, groups: [{ ...data.groups[0], personId: 'p', members: [a, b] }] };
  await act(async () => client.setQueryData(['people-workspace', 'C:/photos'], next));
  await gridSelection([a.assetPath, b.assetPath]);
  const filter = host.querySelector<HTMLSelectElement>('[aria-label="实例审阅过滤"]')!;
  await act(async () => { filter.value = 'pending'; filter.dispatchEvent(new Event('change', { bubbles: true })); });
  expect(host.querySelector('[aria-label="共享人物已选实例数"]')?.textContent).toBe('1');
  expect(button('批量确认为 共享人物').disabled).toBe(true);
  await act(async () => button('批量不属于').click());
  expect(api.review).toHaveBeenCalledWith(next, [a], 'p', 'doesNotBelong', false);
});
