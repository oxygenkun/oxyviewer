import { describe, expect, it } from "vitest";
import type { AssetTagAssignment, CustomTag } from "@/types";
import {
  collectPersonTagIds,
  sortTagCandidates,
  descendantIds,
  embeddedOnlyTagPaths,
  ensureTagPath,
  flattenTags,
  parseTagPath,
  normalizeCustomTag,
} from "@/lib/assets/tagTree";

const tags: CustomTag[] = [
  { id: 1, name: "People", path: "People", sortOrder: 0 },
  { id: 2, parentId: 1, name: "Family", path: "People|Family", sortOrder: 0 },
  { id: 3, parentId: 2, name: "Portraits", path: "People|Family|Portraits", sortOrder: 0 },
  { id: 4, name: "Places", path: "Places", sortOrder: 1 },
];

const assignments: AssetTagAssignment[] = tags.map((tag) => ({
  tag,
  assignedCount: tag.id === 3 ? 1 : 0,
  assetCount: 2,
}));

describe("hierarchical tag presentation", () => {
  it("identifies person categories and every descendant without global person bindings", () => {
    const categories: CustomTag[] = [
      ...tags,
      { id: 5, parentId: 4, name: "人物", path: "Places|人物", sortOrder: 0 },
      { id: 6, parentId: 5, name: "小明", path: "Places|人物|小明", sortOrder: 0 },
      { id: 7, name: "人物摄影", path: "人物摄影", sortOrder: 2 },
      { id: 8, name: "People watching", path: "People watching", sortOrder: 3 },
    ];
    expect([...collectPersonTagIds(categories, [])].sort()).toEqual([1, 2, 3, 5, 6]);
  });

  it("hides descendants of a bound tag without hiding its parent or siblings", () => {
    const categories: CustomTag[] = [
      { id: 1, name: "Categories", path: "Categories", sortOrder: 0 },
      { id: 2, parentId: 1, name: "Alice", path: "Categories|Alice", sortOrder: 0 },
      { id: 3, parentId: 2, name: "Family", path: "Categories|Alice|Family", sortOrder: 0 },
      { id: 4, parentId: 1, name: "Places", path: "Categories|Places", sortOrder: 1 },
    ];
    expect([...collectPersonTagIds(categories, [2])]).toEqual([2, 3]);
  });

  it("shows only roots until their ancestors are expanded", () => {
    expect(flattenTags(assignments, new Set(), "").map((item) => item.assignment.tag.name))
      .toEqual(["People", "Places"]);
    expect(flattenTags(assignments, new Set([1, 2]), "").map((item) => item.assignment.tag.name))
      .toEqual(["People", "Family", "Portraits", "Places"]);
  });

  it("keeps matching paths and their ancestors during search", () => {
    expect(flattenTags(assignments, new Set(), "portrait").map((item) => item.assignment.tag.name))
      .toEqual(["People", "Family", "Portraits"]);
  });

  it("identifies every descendant for safe move choices", () => {
    expect([...descendantIds(tags, 1)]).toEqual([1, 2, 3]);
  });

  it("recognizes slash, backslash, and pipe as hierarchy separators", () => {
    expect(parseTagPath(" 人物 / 家人\\孩子 | 生日 ")).toEqual(["人物", "家人", "孩子", "生日"]);
  });

  it("reuses existing ancestors and creates only the missing path", async () => {
    const created: Array<[number | undefined, string]> = [];
    const result = await ensureTagPath("people/family/Newborn", tags, async (parentId, name) => {
      created.push([parentId, name]);
      return { id: 10, parentId, name, path: `People|Family|${name}`, sortOrder: 1 };
    });
    expect(created).toEqual([[2, "Newborn"]]);
    expect(result.id).toBe(10);
    expect(result.path).toBe("People|Family|Newborn");
  });

  it("reuses null-parent desktop roots for multiple sibling paths", async () => {
    const available = tags.map((tag) => normalizeCustomTag({ ...tag, parentId: tag.parentId ?? null }));
    const created: string[] = [];
    for (const name of ["Newborn", "Holiday"]) {
      await ensureTagPath(`People/Family/${name}`, available, async (parentId, childName) => {
        expect(parentId).toBe(2);
        created.push(childName);
        const tag = { id: 10 + created.length, parentId, name: childName, path: `People|Family|${childName}`, sortOrder: 0 };
        available.push(tag);
        return tag;
      });
    }
    expect(created).toEqual(["Newborn", "Holiday"]);
    expect(await ensureTagPath("People/Family/Holiday", available, async () => {
      throw new Error("Existing path must not be created again");
    })).toMatchObject({ name: "Holiday" });
  });

  it("deduplicates embedded tags and leaves embedded-only values read-only", () => {
    expect(embeddedOnlyTagPaths(
      [tags[1]],
      ["Family", "camera-only"],
      ["People|Family", "Places|Tokyo"],
    )).toEqual(["Places|Tokyo", "camera-only"]);
  });
});

it("sorts candidates by global usage, recent assignment, and sibling names", () => {
  const items = [
    { tag: { id: 1, name: "B", path: "B", sortOrder: 0, usageCount: 10, lastUsedAt: 10 }, assignedCount: 0, assetCount: 1 },
    { tag: { id: 2, name: "Z", path: "A|Z", parentId: 3, sortOrder: 0, usageCount: 2, lastUsedAt: 30 }, assignedCount: 1, assetCount: 1 },
    { tag: { id: 3, name: "A", path: "A", sortOrder: 9 }, assignedCount: 0, assetCount: 1 },
    { tag: { id: 4, name: "Aardvark", path: "Aardvark", sortOrder: 0 }, assignedCount: 0, assetCount: 1 },
  ];
  const ids = (mode: "tree" | "recent" | "mostUsed") => sortTagCandidates(items, mode).map(({ tag }) => tag.id);
  expect(ids("tree")).toEqual([3, 2, 4, 1]);
  expect(ids("recent")).toEqual([2, 1, 3, 4]);
  expect(ids("mostUsed")).toEqual([1, 2, 3, 4]);
  expect(items.map(({ tag }) => tag.id)).toEqual([1, 2, 3, 4]);
});
