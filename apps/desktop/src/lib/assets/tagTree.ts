import type { AssetTagAssignment, CustomTag } from "@/types";

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

/** Hide person categories and their descendants even before person sync is configured. */
export function collectPersonTagIds(tags: CustomTag[], boundTagIds: Iterable<number>): Set<number> {
  const result = new Set(boundTagIds);
  const children = new Map<number, number[]>();
  for (const tag of tags) {
    const name = tag.name.trim().toLocaleLowerCase();
    if (name === "人物" || name === "people") result.add(tag.id);
    if (tag.parentId != null) {
      const siblings = children.get(tag.parentId) ?? [];
      siblings.push(tag.id);
      children.set(tag.parentId, siblings);
    }
  }
  const pending = [...result];
  for (let index = 0; index < pending.length; index++) {
    for (const id of children.get(pending[index]) ?? []) {
      if (result.has(id)) continue;
      result.add(id);
      pending.push(id);
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

// Rust serializes a root parent as null; keep the frontend tree identity consistent.
export function normalizeCustomTag(tag: Omit<CustomTag, "parentId"> & { parentId?: number | null }): CustomTag {
  return { ...tag, parentId: tag.parentId ?? undefined };
}

export type TagPickerSort = "recent" | "mostUsed" | "tree";

/** Compare path segments so parents precede children and siblings stay together. */
export function compareTagPaths(left: CustomTag, right: CustomTag): number {
  const a = left.path.split("|");
  const b = right.path.split("|");
  for (let i = 0; i < Math.min(a.length, b.length); i++) {
    const order = a[i].localeCompare(b[i], undefined, { numeric: true, sensitivity: "base" });
    if (order) return order;
  }
  return a.length - b.length || left.id - right.id;
}

export function sortTagCandidates(assignments: AssetTagAssignment[], mode: TagPickerSort): AssetTagAssignment[] {
  return [...assignments].sort((a, b) => {
    const rank = mode === "recent"
      ? (b.tag.lastUsedAt ?? 0) - (a.tag.lastUsedAt ?? 0)
      : mode === "mostUsed" ? (b.tag.usageCount ?? 0) - (a.tag.usageCount ?? 0) : 0;
    return rank || compareTagPaths(a.tag, b.tag);
  });
}
