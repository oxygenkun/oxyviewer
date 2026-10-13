// @vitest-environment jsdom
import { notifyManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createCustomTag, deleteCustomTag, getAssetTagAssignments, setAssetCustomTag } from "@/lib/api";
import { TAG_MUTATION_SCOPE } from "@/lib/assets/tagActions";
import { useWorkspaceStore } from "@/store";
import type { CustomTag } from "@/types";
import { TagEditor } from "./TagEditor";

const tags: CustomTag[] = [
  { id: 1, name: "People", path: "People", sortOrder: 0 },
  { id: 2, parentId: 1, name: "Family", path: "People|Family", sortOrder: 0 },
  { id: 3, name: "Places", path: "Places", sortOrder: 1 },
];

vi.mock("@/lib/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...original,
    listCustomTags: vi.fn(async () => tags),
    getAssetTagAssignments: vi.fn(),
    getAssetTagSourceKinds: vi.fn(async () => ["manual"]),
    getTagSyncStatus: vi.fn(async () => ({ pendingCount: 0, failedCount: 0 })),
    getCustomTagDeleteImpact: vi.fn(async () => ({ tagCount: 2, assetCount: 1 })),
    setAssetCustomTag: vi.fn(),
    createCustomTag: vi.fn(),
    deleteCustomTag: vi.fn(async () => undefined),
  };
});

let root: Root;
let host: HTMLDivElement;
let client: QueryClient;
let assigned: Set<number>;

const click = async (selector: string) => {
  const button = document.querySelector<HTMLButtonElement>(selector)!;
  expect(button).not.toBeNull();
  expect(button.disabled).toBe(false);
  await act(async () => button.click());
};
const renderEditor = async (path = "/demo/a.jpg") => {
  await act(async () => root.render(
    <QueryClientProvider client={client}>
      <TagEditor currentPath={path} paths={[path]} sidecarKeywords={[]} sidecarHierarchicalKeywords={[]}
        embeddedKeywords={[]} embeddedHierarchicalKeywords={[]} t={(key) => key} />
    </QueryClientProvider>,
  ));
  await vi.waitFor(() => expect(client.isFetching()).toBe(0));
};

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  notifyManager.setNotifyFunction((callback) => { act(callback); });
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  useWorkspaceStore.setState({ lastTagId: undefined });
  assigned = new Set();
  vi.mocked(getAssetTagAssignments).mockImplementation(async (paths) => tags.map((tag) => ({
    tag, assignedCount: assigned.has(tag.id) ? paths.length : 0, assetCount: paths.length,
  })));
  vi.mocked(setAssetCustomTag).mockReset().mockImplementation(async (_paths, tagId, value) => {
    if (value) assigned.add(tagId); else assigned.delete(tagId);
  });
  vi.mocked(createCustomTag).mockReset().mockResolvedValue({ id: 4, name: "New", path: "New", sortOrder: 2 });
  vi.mocked(deleteCustomTag).mockClear();
  client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await renderEditor();
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  client.clear();
  notifyManager.setNotifyFunction((callback) => { callback(); });
  vi.unstubAllGlobals();
});

it("remembers successful assignments and removals across photos", async () => {
  await click(".tag-editor__quick-add");
  await click("[role=checkbox][aria-label='People › Family']");
  await vi.waitFor(() => expect(useWorkspaceStore.getState().lastTagId).toBe(2));
  await renderEditor("/demo/b.jpg");
  expect(useWorkspaceStore.getState().lastTagId).toBe(2);
  await vi.waitFor(() => expect(document.querySelector("[role=checkbox][aria-label='Places']")?.hasAttribute("disabled")).toBe(false));
  await click("[role=checkbox][aria-label='Places']");
  await vi.waitFor(() => expect(useWorkspaceStore.getState().lastTagId).toBe(3));
  await vi.waitFor(() => expect(document.querySelector(".tag-editor__chips [aria-label='removeTagFromCurrent People › Family']")).not.toBeNull());
  await click(".tag-editor__chips [aria-label='removeTagFromCurrent People › Family']");
  await vi.waitFor(() => expect(document.querySelector<HTMLButtonElement>(".tag-remove-confirmation .is-danger")?.disabled).toBe(false));
  await click(".tag-remove-confirmation .is-danger");
  await vi.waitFor(() => expect(useWorkspaceStore.getState().lastTagId).toBe(2));
  expect(setAssetCustomTag).toHaveBeenLastCalledWith(["/demo/b.jpg"], 2, false);
});

it("remembers the newly created leaf tag after assigning it", async () => {
  await click(".tag-editor__quick-add");
  await click(".tag-picker__create");
  const input = document.querySelector<HTMLInputElement>(".tag-picker__create-form input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "New");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click("[aria-label=createAndAdd]");
  await vi.waitFor(() => expect(useWorkspaceStore.getState().lastTagId).toBe(4));
  expect(setAssetCustomTag).toHaveBeenCalledWith(["/demo/a.jpg"], 4, true);
});

it("keeps the previous tag when an assignment fails", async () => {
  useWorkspaceStore.setState({ lastTagId: 3 });
  vi.mocked(setAssetCustomTag).mockRejectedValueOnce(new Error("write failed"));
  await click(".tag-editor__quick-add");
  await click("[role=checkbox][aria-label='People › Family']");
  await vi.waitFor(() => expect(document.querySelector(".metadata-error")?.textContent).toContain("write failed"));
  expect(useWorkspaceStore.getState().lastTagId).toBe(3);
});

it("keeps a queued edit on the photo selected when it was triggered", async () => {
  let release!: () => void;
  const blocker = client.getMutationCache().build(client, {
    scope: TAG_MUTATION_SCOPE,
    mutationFn: () => new Promise<void>((resolve) => { release = resolve; }),
  });
  const pending = blocker.execute(undefined);
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  await click(".tag-editor__quick-add");
  await click("[role=checkbox][aria-label='People › Family']");
  await renderEditor("/demo/b.jpg");
  expect(setAssetCustomTag).not.toHaveBeenCalled();
  await act(async () => { release(); await pending; });
  await vi.waitFor(() => expect(setAssetCustomTag).toHaveBeenCalledWith(["/demo/a.jpg"], 2, true));
});

it("clears the remembered child tag after its parent is deleted", async () => {
  useWorkspaceStore.setState({ lastTagId: 2 });
  await click(".tag-editor__quick-add");
  await click(".tag-candidate:has([aria-label=People]) .tag-candidate__menu");
  await click(".tag-row-menu .is-danger");
  await vi.waitFor(() => expect(document.querySelector<HTMLButtonElement>(".tag-picker--confirmation .is-danger")?.disabled).toBe(false));
  await click(".tag-picker--confirmation .is-danger");
  await vi.waitFor(() => expect(useWorkspaceStore.getState().lastTagId).toBeUndefined());
  expect(deleteCustomTag).toHaveBeenCalledWith(1);
});
