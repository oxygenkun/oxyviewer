import { describe, expect, it } from "vitest";
import { buildDirectorySearchTree } from "./directorySearchTree";

describe("buildDirectorySearchTree", () => {
  it("keeps only the ancestors needed to reach matching folders", () => {
    const tree = buildDirectorySearchTree(
      { path: "/photos", name: "photos", hasChildren: true },
      [
        {
          directory: { path: "/photos/2026/Team A", name: "Team A", hasChildren: false },
          ancestors: [{ path: "/photos/2026", name: "2026", hasChildren: true }],
        },
        {
          directory: { path: "/photos/2025/Team A", name: "Team A", hasChildren: false },
          ancestors: [{ path: "/photos/2025", name: "2025", hasChildren: true }],
        },
      ],
    );

    expect(tree.children.map((node) => node.entry.name)).toEqual(["2025", "2026"]);
    expect(tree.children[0].children[0]).toMatchObject({
      entry: { name: "Team A" },
      matched: true,
    });
  });

  it("merges shared ancestors and marks ancestors that also match", () => {
    const tree = buildDirectorySearchTree(
      { path: "/photos", name: "photos", hasChildren: true },
      [
        {
          directory: { path: "/photos/Team", name: "Team", hasChildren: true },
          ancestors: [],
        },
        {
          directory: { path: "/photos/Team/Team Blue", name: "Team Blue", hasChildren: false },
          ancestors: [{ path: "/photos/Team", name: "Team", hasChildren: true }],
        },
      ],
    );

    expect(tree.children).toHaveLength(1);
    expect(tree.children[0].matched).toBe(true);
    expect(tree.children[0].children[0].matched).toBe(true);
  });
});
