/**
 * Orders items around the selection for every frontend image queue:
 * selected, nearest right, nearest left, remaining right, remaining left.
 */
export function orderBySelectionPriority<T>(
  items: readonly T[],
  selectedId: string,
  getId: (item: T) => string,
): T[] {
  const selectedIndex = items.findIndex((item) => getId(item) === selectedId);
  if (selectedIndex < 0) return [...items];

  const selected = items[selectedIndex];
  const right = items.slice(selectedIndex + 1);
  const left = items.slice(0, selectedIndex).reverse();
  return [selected, right[0], left[0], ...right.slice(1), ...left.slice(1)].filter(
    (item): item is T => item !== undefined,
  );
}

/** Alternates right/left around the selection, then drains the longer side. */
export function orderBalancedAroundSelection<T>(
  items: readonly T[],
  selectedId: string,
  getId: (item: T) => string,
): T[] {
  const selectedIndex = items.findIndex((item) => getId(item) === selectedId);
  if (selectedIndex < 0) return [...items];

  const ordered: T[] = [];
  const left = items.slice(0, selectedIndex).reverse();
  const right = items.slice(selectedIndex + 1);
  const pairedCount = Math.min(left.length, right.length);
  for (let index = 0; index < pairedCount; index += 1) {
    ordered.push(right[index], left[index]);
  }
  ordered.push(...right.slice(pairedCount), ...left.slice(pairedCount));
  return ordered;
}
