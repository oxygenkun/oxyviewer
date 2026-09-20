import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { scanBurstGroups } from "@/lib/api";
import type { AssetSummary, BurstGroup } from "@/types";

/**
 * Paths a collapsed burst hides: every member but the representative.
 *
 * Expanding a group keeps its members in the grid; collapsing it again pulls
 * them back out.
 */
export function collapsedBurstPaths(
  groups: BurstGroup[],
  expanded: ReadonlySet<string>,
  enabled = true,
): Set<string> {
  const hidden = new Set<string>();
  if (!enabled) return hidden;
  for (const group of groups) {
    if (expanded.has(group.representative)) continue;
    for (const path of group.members.slice(1)) hidden.add(path);
  }
  return hidden;
}

/**
 * Estimate the virtual list total while pages are still loading.
 *
 * The backend total counts frames, so every collapsed member already found in
 * the loaded prefix must be removed from it. Once every page is loaded this is
 * the exact number of visible grid tiles.
 */
export function burstAdjustedTotal(
  total: number,
  loadedCount: number,
  visibleLoadedCount: number,
): number {
  const collapsedLoadedCount = Math.max(0, loadedCount - visibleLoadedCount);
  return Math.max(visibleLoadedCount, total - collapsedLoadedCount);
}

export interface BurstGrouping {
  /** Groups found so far, in browsing order. */
  groups: BurstGroup[];
  /** The listing with collapsed burst members removed. */
  visible: AssetSummary[];
  /** Groups by every member path, so expanded tiles retain their group UI. */
  byMember: Map<string, BurstGroup>;
  expanded: ReadonlySet<string>;
  toggle: (representative: string) => void;
}

/**
 * Groups the loaded assets into bursts, collapsing each run to its first frame.
 *
 * The scan is incremental: Rust parses only the paths it has not seen, so a
 * newly loaded page costs just its own files. Nothing is read until the first
 * page is on screen, which keeps the folder open path free of maker note work.
 */
export function useBurstGroups(assets: AssetSummary[], enabled = true): BurstGrouping {
  const [groups, setGroups] = useState<BurstGroup[]>([]);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const scannedPathsRef = useRef<string[]>([]);
  const scannedCountRef = useRef(0);

  useEffect(() => {
    if (!enabled || !assets.length) {
      setGroups([]);
      setExpanded(new Set());
      scannedPathsRef.current = [];
      scannedCountRef.current = 0;
      return;
    }
    const paths = assets.map((asset) => asset.path);
    const previousPaths = scannedPathsRef.current;
    const extendsPreviousScan = previousPaths.length <= paths.length
      && previousPaths.every((path, index) => paths[index] === path);
    if (!extendsPreviousScan) {
      setGroups([]);
      setExpanded(new Set());
      scannedCountRef.current = 0;
    }
    scannedPathsRef.current = paths;
    let cancelled = false;
    const scanProgressively = async () => {
      let scannedCount = extendsPreviousScan ? scannedCountRef.current : 0;
      while (!cancelled && scannedCount < paths.length) {
        const batchSize = scannedCount === 0 ? 24 : 64;
        const nextCount = Math.min(paths.length, scannedCount + batchSize);
        try {
          const next = await scanBurstGroups(paths.slice(0, nextCount));
          if (cancelled) return;
          scannedCount = nextCount;
          scannedCountRef.current = nextCount;
          setGroups(next);
        } catch {
          return;
        }
      }
    };
    void scanProgressively();
    return () => {
      cancelled = true;
    };
  }, [assets, enabled]);

  const hidden = useMemo(
    () => collapsedBurstPaths(groups, expanded, enabled),
    [enabled, expanded, groups],
  );
  const visible = useMemo(
    () => (hidden.size ? assets.filter((asset) => !hidden.has(asset.path)) : assets),
    [assets, hidden],
  );
  const byMember = useMemo(() => {
    const map = new Map<string, BurstGroup>();
    if (!enabled) return map;
    for (const group of groups) {
      for (const path of group.members) map.set(path, group);
    }
    return map;
  }, [enabled, groups]);
  const toggle = useCallback((representative: string) => {
    if (!enabled) return;
    setExpanded((previous) => {
      const next = new Set(previous);
      if (!next.delete(representative)) next.add(representative);
      return next;
    });
  }, [enabled]);

  return { groups: enabled ? groups : [], visible, byMember, expanded, toggle };
}
