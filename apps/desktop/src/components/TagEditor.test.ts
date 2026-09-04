import { describe, expect, it } from "vitest";
import type { AssetTagAssignment, CustomTag } from "../types";
import {
  descendantIds,
  embeddedOnlyTagPaths,
  ensureTagPath,
  flattenTags,
  parseTagPath,
} from "../lib/tagTree";

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

  it("deduplicates embedded tags and leaves embedded-only values read-only", () => {
    expect(embeddedOnlyTagPaths(
      [tags[1]],
      ["Family", "camera-only"],
      ["People|Family", "Places|Tokyo"],
    )).toEqual(["Places|Tokyo", "camera-only"]);
  });
});
