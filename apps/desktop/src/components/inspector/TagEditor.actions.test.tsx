// @vitest-environment jsdom
import { notifyManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createCustomTag, deleteCustomTag, getAssetTagAssignments, getAssetTagSourceKinds, setAssetCustomTag } from "@/lib/api";
import { TAG_MUTATION_SCOPE } from "@/lib/assets/tagActions";
import { cancelPendingTagDeletions, useTagDeletions } from "@/lib/assets/tagDeletion";
import { UNDO_WINDOW_MS } from "@/lib/ui/undoWindow";
import { TagDeletionToasts } from "@/components/overlay/TagDeletionToasts";
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
  expect(button.matches(":disabled")).toBe(false);
  await act(async () => {
    if (button.getAttribute("aria-haspopup") === "menu") {
      button.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    } else button.click();
  });
};
const renderEditor = async (path = "/demo/a.jpg") => {
  await act(async () => root.render(
    <QueryClientProvider client={client}>
      <TagEditor currentPath={path} paths={[path]} sidecarKeywords={[]} sidecarHierarchicalKeywords={[]}
        embeddedKeywords={[]} embeddedHierarchicalKeywords={[]} t={(key) => key} />
      <TagDeletionToasts t={(key) => key} />
    </QueryClientProvider>,
  ));
  await vi.waitFor(() => expect(client.isFetching()).toBe(0));
};

const chipSelector = "removeTagFromCurrent People › Family";
/** Let the deferred write of a pending deletion run without waiting the real undo window. */
const advanceUndoWindow = async () => {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(UNDO_WINDOW_MS);
    await Promise.resolve();
  });
  await act(async () => { await Promise.resolve(); });
};

beforeEach(async () => {
  localStorage.clear();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  notifyManager.setNotifyFunction((callback) => { act(callback); });
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  useWorkspaceStore.setState({ lastTagId: undefined });
  useTagDeletions.setState({ requests: [] });
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
  cancelPendingTagDeletions();
  useTagDeletions.setState({ requests: [] });
  host.remove();
  client.clear();
  notifyManager.setNotifyFunction((callback) => { callback(); });
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it("remembers successful assignments and defers removals", async () => {
  await click(".tag-editor__quick-add");
  await click("[role=checkbox][aria-label='People › Family']");
  await vi.waitFor(() => expect(useWorkspaceStore.getState().lastTagId).toBe(2));
  await renderEditor("/demo/b.jpg");
  expect(useWorkspaceStore.getState().lastTagId).toBe(2);
  await vi.waitFor(() => expect(document.querySelector("[role=checkbox][aria-label='Places']")?.hasAttribute("disabled")).toBe(false));
  await click("[role=checkbox][aria-label='Places']");
  await vi.waitFor(() => expect(useWorkspaceStore.getState().lastTagId).toBe(3));
  await vi.waitFor(() => expect(document.querySelector(`.tag-editor__chips [aria-label='${chipSelector}']`)).not.toBeNull());
  await click(`.tag-editor__chips [aria-label='${chipSelector}']`);
  await vi.waitFor(() => expect(document.querySelector(`.tag-editor__chips [aria-label='${chipSelector}']`)).toBeNull());
  expect(document.querySelector(".toast")?.textContent).toContain("tagRemovalPending");
  expect(setAssetCustomTag).not.toHaveBeenCalledWith(["/demo/b.jpg"], 2, false);
});

it("remembers the newly created leaf tag after assigning it", async () => {
  await click(".tag-editor__quick-add");
  expect(document.querySelector(".tag-picker__create")).toBeNull();
  const input = document.querySelector<HTMLInputElement>(".tag-picker__search input")!;
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

it("deletes a parent tag after the undo window and forgets its child", async () => {
  useWorkspaceStore.setState({ lastTagId: 2 });
  await click(".tag-editor__quick-add");
  await click(".tag-candidate:has([aria-label=People]) .tag-candidate__menu");
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  try {
    await click('.tag-row-menu [data-variant="danger"]');
    expect(deleteCustomTag).not.toHaveBeenCalled();
    expect(document.querySelector(".tag-picker--confirmation")).toBeNull();
    expect(document.querySelector("[role=checkbox][aria-label='People']")).toBeNull();
    expect(document.querySelector(".toast")?.textContent).toContain("tagDeletionPending");
    await advanceUndoWindow();
    expect(deleteCustomTag).toHaveBeenCalledWith(1);
    expect(useWorkspaceStore.getState().lastTagId).toBeUndefined();
    expect(document.querySelector(".toast")).toBeNull();
  } finally {
    vi.useRealTimers();
  }
});

it("removes a tag from the current photo after the undo window", async () => {
  assigned.add(2);
  await act(async () => { await client.invalidateQueries({ queryKey: ["asset-tag-assignments"] }); });
  await vi.waitFor(() => expect(document.querySelector(`.tag-editor__chips [aria-label='${chipSelector}']`)).not.toBeNull());
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  try {
    await click(`.tag-editor__chips [aria-label='${chipSelector}']`);
    expect(document.querySelector(`.tag-editor__chips [aria-label='${chipSelector}']`)).toBeNull();
    expect(setAssetCustomTag).not.toHaveBeenCalledWith(["/demo/a.jpg"], 2, false);
    await advanceUndoWindow();
    expect(setAssetCustomTag).toHaveBeenCalledWith(["/demo/a.jpg"], 2, false);
    expect(useWorkspaceStore.getState().lastTagId).toBe(2);
    expect(document.querySelector(".toast")).toBeNull();
  } finally {
    vi.useRealTimers();
  }
});

it("restores the tag when the removal is undone before it commits", async () => {
  assigned.add(2);
  await act(async () => { await client.invalidateQueries({ queryKey: ["asset-tag-assignments"] }); });
  await vi.waitFor(() => expect(document.querySelector(`.tag-editor__chips [aria-label='${chipSelector}']`)).not.toBeNull());
  await click(`.tag-editor__chips [aria-label='${chipSelector}']`);
  await vi.waitFor(() => expect(document.querySelector(`.tag-editor__chips [aria-label='${chipSelector}']`)).toBeNull());
  await click(".toast button");
  expect(document.querySelector(`.tag-editor__chips [aria-label='${chipSelector}']`)).not.toBeNull();
  expect(document.querySelector(".toast")).toBeNull();
  expect(setAssetCustomTag).not.toHaveBeenCalledWith(["/demo/a.jpg"], 2, false);
});

it("explains person synced tags instead of removing them", async () => {
  assigned.add(2);
  await act(async () => { await client.invalidateQueries({ queryKey: ["asset-tag-assignments"] }); });
  await vi.waitFor(() => expect(document.querySelector(`.tag-editor__chips [aria-label='${chipSelector}']`)).not.toBeNull());
  vi.mocked(getAssetTagSourceKinds).mockResolvedValueOnce(["person"]);
  await click(`.tag-editor__chips [aria-label='${chipSelector}']`);
  await vi.waitFor(() => expect(document.querySelector(".tag-editor__hint")?.textContent).toBe("tagPersonSourceHint"));
  expect(document.querySelector(`.tag-editor__chips [aria-label='${chipSelector}']`)).not.toBeNull();
  expect(document.querySelector(".toast")).toBeNull();
});

it("switches picker sorting while preserving selection", async () => {
  vi.mocked(getAssetTagAssignments).mockImplementation(async () => tags.map((tag) => ({
    tag: { ...tag, usageCount: tag.id === 3 ? 20 : 1, lastUsedAt: tag.id === 2 ? 100 : 0 },
    assignedCount: tag.id === 2 ? 1 : 0, assetCount: 1,
  })));
  await act(async () => { await client.invalidateQueries({ queryKey: ["asset-tag-assignments"] }); });
  await click(".tag-editor__quick-add");
  const names = () => [...document.querySelectorAll(".tag-candidate__name")].map((node) => node.textContent);
  expect(names()).toEqual(["People", "Family", "Places"]);
  await click(".tag-picker__settings");
  await click('[role="menuitemradio"][data-state="unchecked"]:nth-child(2)');
  expect(names()).toEqual(["Places", "People", "People › Family"]);
  await click(".tag-picker__settings");
  await click('[role="menuitemradio"]:first-child');
  expect(names()[0]).toBe("People › Family");
  expect(localStorage.getItem("oxyviewer.tag-picker-sort.v1")).toBe("recent");
  await act(async () => root.render(null));
  await renderEditor();
  await click(".tag-editor__quick-add");
  await click(".tag-picker__settings");
  expect(document.querySelector('[role="menuitemradio"][aria-checked="true"]')?.textContent).toBe("tagSortRecent");
  await act(async () => document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
  expect(names()[0]).toBe("People › Family");
  expect(document.querySelector('[role=checkbox][aria-label="People › Family"]')?.getAttribute("aria-checked")).toBe("true");
});

it("offers direct creation only for a new nonempty tag and preserves input on failure", async () => {
  await click(".tag-editor__quick-add");
  expect(document.querySelector(".tag-picker__create")).toBeNull();
  const input = document.querySelector<HTMLInputElement>(".tag-picker__search input")!;
  const type = async (value: string) => act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await type("family");
  expect(document.querySelector(".tag-picker__create")).toBeNull();
  await type(" / | ");
  expect(document.querySelector(".tag-picker__create")).toBeNull();
  await type("People/New");
  expect(document.querySelector(".tag-picker__search .tag-picker__create")).not.toBeNull();
  vi.mocked(createCustomTag).mockRejectedValueOnce(new Error("create failed"));
  await click("[aria-label=createAndAdd]");
  await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("create failed"));
  expect(input.value).toBe("People/New");
  expect(setAssetCustomTag).not.toHaveBeenCalled();
});

it("dismisses only the nested menu on Escape and keeps rename focus inside the picker", async () => {
  await click(".tag-editor__quick-add");
  await click(".tag-picker__settings");
  const option = document.querySelector<HTMLElement>('[role="menuitemradio"]')!;
  await act(async () => option.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
  expect(document.querySelector('[role="menu"]')).toBeNull();
  expect(document.querySelector(".tag-picker")).not.toBeNull();
  await click(".tag-candidate__menu");
  await click('.tag-row-menu [role="menuitem"]:first-child');
  await vi.waitFor(() => expect(document.activeElement).toBe(document.querySelector(".tag-edit-panel input")));
  expect(document.querySelector(".tag-picker")).not.toBeNull();
  expect(setAssetCustomTag).not.toHaveBeenCalled();
});
