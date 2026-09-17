import type { DirectorySearchMatch, DirectorySummary } from "@/types";

export interface DirectorySearchTreeNode {
  entry: DirectorySummary;
  matched: boolean;
  children: DirectorySearchTreeNode[];
}

export function buildDirectorySearchTree(
  root: DirectorySummary,
  matches: DirectorySearchMatch[],
): DirectorySearchTreeNode {
  const rootNode: DirectorySearchTreeNode = { entry: root, matched: false, children: [] };
  const nodes = new Map<string, DirectorySearchTreeNode>([[root.path, rootNode]]);

  for (const match of matches) {
    let parent = rootNode;
    for (const entry of [...match.ancestors, match.directory]) {
      let node = nodes.get(entry.path);
      if (!node) {
        node = { entry, matched: false, children: [] };
        nodes.set(entry.path, node);
        parent.children.push(node);
      }
      parent = node;
    }
    parent.matched = true;
  }

  sortTree(rootNode);
  return rootNode;
}

function sortTree(node: DirectorySearchTreeNode) {
  node.children.sort((left, right) => left.entry.name.localeCompare(right.entry.name));
  node.children.forEach(sortTree);
}
