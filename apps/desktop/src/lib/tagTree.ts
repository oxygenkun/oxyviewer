import type { AssetTagAssignment, CustomTag } from "../types";

export interface VisibleTag {
  assignment: AssetTagAssignment;
  depth: number;
  hasChildren: boolean;
}

export function flattenTags(
  assignments: AssetTagAssignment[],
  expanded: Set<number>,
  search: string,
): VisibleTag[] {
  const children = new Map<number | undefined, AssetTagAssignment[]>();
  for (const assignment of assignments) {
    const siblings = children.get(assignment.tag.parentId) ?? [];
    siblings.push(assignment);
    children.set(assignment.tag.parentId, siblings);
  }
  for (const siblings of children.values()) {
    siblings.sort((a, b) => a.tag.sortOrder - b.tag.sortOrder || a.tag.name.localeCompare(b.tag.name));
  }
  const needle = search.trim().toLocaleLowerCase();
  const visible = new Set<number>();
  if (needle) {
    const byId = new Map(assignments.map((item) => [item.tag.id, item.tag]));
    for (const { tag } of assignments) {
      if (!tag.path.toLocaleLowerCase().includes(needle)) continue;
      let current: CustomTag | undefined = tag;
      while (current) {
        visible.add(current.id);
        current = current.parentId === undefined ? undefined : byId.get(current.parentId);
      }
    }
  }
  const result: VisibleTag[] = [];
  const visit = (parentId: number | undefined, depth: number) => {
    for (const assignment of children.get(parentId) ?? []) {
      if (needle && !visible.has(assignment.tag.id)) continue;
      const hasChildren = (children.get(assignment.tag.id)?.length ?? 0) > 0;
      result.push({ assignment, depth, hasChildren });
      if (needle || expanded.has(assignment.tag.id)) visit(assignment.tag.id, depth + 1);
    }
  };
  visit(undefined, 0);
  return result;
}

export function descendantIds(tags: CustomTag[], id: number): Set<number> {
  const result = new Set([id]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const tag of tags) {
      if (tag.parentId !== undefined && result.has(tag.parentId) && !result.has(tag.id)) {
        result.add(tag.id);
        changed = true;
      }
    }
  }
  return result;
}

const TAG_PATH_SEPARATOR = /[\\/|]+/;

export function parseTagPath(value: string): string[] {
  return value
    .split(TAG_PATH_SEPARATOR)
    .map((segment) => segment.trim())
    .filter(Boolean);
}

export async function ensureTagPath(
  value: string,
  tags: CustomTag[],
  createTag: (parentId: number | undefined, name: string) => Promise<CustomTag>,
): Promise<CustomTag> {
  const segments = parseTagPath(value);
  if (segments.length === 0) throw new Error("Tag path cannot be empty");

  const available = [...tags];
  let parentId: number | undefined;
  let current: CustomTag | undefined;
  for (const segment of segments) {
    const nameKey = segment.toLocaleLowerCase();
    current = available.find((tag) =>
      tag.parentId === parentId && tag.name.toLocaleLowerCase() === nameKey,
    );
    if (!current) {
      current = await createTag(parentId, segment);
      available.push(current);
    }
    parentId = current.id;
  }
  return current!;
}

export function embeddedOnlyTagPaths(
  managedTags: CustomTag[],
  keywords: string[],
  hierarchicalKeywords: string[],
): string[] {
  const identity = (value: string) => value.toLocaleLowerCase();
  const managedPaths = new Set(managedTags.map((tag) => identity(tag.path)));
  const managedLeafNames = new Set(managedTags.map((tag) => identity(tag.name)));
  const hierarchicalLeaves = new Set(
    hierarchicalKeywords.map((path) => identity(path.split("|").at(-1) ?? path)),
  );
  const embedded = [
    ...hierarchicalKeywords.map((path) => ({ path, hierarchical: true })),
    ...keywords
      .filter((keyword) => !hierarchicalLeaves.has(identity(keyword)))
      .map((path) => ({ path, hierarchical: false })),
  ];
  const seen = new Set<string>();
  return embedded.filter(({ path, hierarchical }) => {
    const pathIdentity = identity(path);
    const leafIdentity = identity(path.split("|").at(-1) ?? path);
    const overlapsManaged = hierarchical
      ? managedPaths.has(pathIdentity)
      : managedLeafNames.has(leafIdentity);
    if (seen.has(pathIdentity) || overlapsManaged) return false;
    seen.add(pathIdentity);
    return true;
  }).map(({ path }) => path);
}
