// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { WorkspaceSearch } from "./WorkspaceSearch";
import { useWorkspaceStore } from "../store";
import type { CustomTag } from "../types";

let client: QueryClient;
let root: Root;
let host: HTMLDivElement;
const original: CustomTag[] = [{ id: 1, name: "Family", path: "People|Family", sortOrder: 0 }];
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  client.setQueryData(["custom-tags"], original);
  useWorkspaceStore.setState({ search: "", tagIds: [1], tagMatch: "all" });
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  await act(async () => root.render(<QueryClientProvider client={client}><WorkspaceSearch t={(key) => key} /></QueryClientProvider>));
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); client.clear(); vi.unstubAllGlobals(); });
async function updateTags(tags: CustomTag[]) {
  await act(async () => { client.setQueryData(["custom-tags"], tags); await new Promise((resolve) => setTimeout(resolve, 10)); });
}
it("keeps IDs on rename/move and removes tokens only after a successful deletion refresh", async () => {
  expect(host.textContent).toContain("People › Family");
  await updateTags([{ ...original[0], name: "Friends", path: "Travel|Friends" }]);
  expect(useWorkspaceStore.getState().tagIds).toEqual([1]);
  expect(host.textContent).toContain("Travel › Friends");
  await updateTags([]);
  expect(useWorkspaceStore.getState().tagIds).toEqual([]);
});
it("clears temporary completion when the workspace clear action fires", async () => {
  const input = host.querySelector("input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "#Family");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(useWorkspaceStore.getState().search).toBe("");
  expect(input.value).toBe("#Family");
  await act(async () => useWorkspaceStore.getState().clearSearch());
  expect(input.value).toBe("");
  expect(useWorkspaceStore.getState().tagIds).toEqual([]);
});
