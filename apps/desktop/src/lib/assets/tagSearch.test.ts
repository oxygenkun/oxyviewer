// @vitest-environment jsdom
import { expect, it } from "vitest";
import { createCustomTag, deleteCustomTag, listAssets, listCustomTags, setAssetCustomTag, updateCustomTag } from "@/lib/api";
import { useWorkspaceStore } from "@/store";

it("combines filename, subtree AND/OR and paging in the browser demo", async () => {
  const query = { sort: "name" as const, direction: "ascending" as const, pageSize: 250 };
  // Use the demo's global filename search to discover its stable fixture paths.
  const all = await listAssets("demo", "/demo", { ...query, search: "." });
  expect(all.items.length).toBeGreaterThan(1);
  const [a, b] = all.items;
  const directory = a.path.slice(0, a.path.lastIndexOf("/"));
  const parent = await createCustomTag(undefined, "Search test parent");
  const child = await createCustomTag(parent.id, "Leaf");
  const other = await createCustomTag(undefined, "Search test other");
  await setAssetCustomTag([a.path], child.id, true);
  await setAssetCustomTag([a.path, b.path], other.id, true);
  const selected = await listAssets("demo", directory, { ...query, tagIds: [parent.id, other.id] });
  expect(selected.items.map((asset) => asset.id)).toEqual([a.id]);
  expect((await listAssets("demo", directory, { ...query, tagIds: [parent.id], search: "does-not-exist" })).total).toBe(0);
  const any = await listAssets("demo", directory, { ...query, tagIds: [parent.id, other.id], tagMatch: "any", pageSize: 1 });
  expect(any.items).toHaveLength(1);
  expect(any.total).toBe([a, b].filter((asset) => asset.path.slice(0, asset.path.lastIndexOf("/")) === directory).length);
  await updateCustomTag(child.id, other.id, "Renamed");
  expect((await listCustomTags()).find((tag) => tag.id === child.id)?.path).toContain("Renamed");
  expect((await listAssets("demo", directory, { ...query, tagIds: [parent.id] })).total).toBe(0);
  expect((await listAssets("demo", directory, { ...query, tagIds: [child.id] })).total).toBe(1);
  await deleteCustomTag(other.id); await deleteCustomTag(parent.id);
  expect((await listAssets("demo", directory, { ...query, tagIds: [child.id] })).total).toBe(0);
});

it("clears only search intent, leaving the second filter row unchanged", () => {
  useWorkspaceStore.setState({ search: "coast", tagIds: [1], kind: "raw", minimumRating: 4, colorLabels: ["Red"], pickLabels: ["accepted"] });
  useWorkspaceStore.getState().clearSearch();
  expect(useWorkspaceStore.getState()).toMatchObject({ search: "", tagIds: [], kind: "raw", minimumRating: 4, colorLabels: ["Red"], pickLabels: ["accepted"] });
});
