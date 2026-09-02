import type { PerfMark } from "../types";

/**
 * Lightweight performance probe for the end-to-end performance harness
 * (docs/PERF_E2E.md). Instrumentation sites call `perfMark`
 * unconditionally; every call is a no-op until `activatePerfProbe` runs, so
 * normal app usage pays only a boolean check per call.
 */
let active = false;
const marks: PerfMark[] = [];
const listeners = new Set<(mark: PerfMark) => void>();

export function activatePerfProbe(): void {
  active = true;
}

export function isPerfActive(): boolean {
  return active;
}

export function perfMark(name: string, detail?: Record<string, unknown>): void {
  if (!active) return;
  const mark: PerfMark = { name, t: performance.now(), detail };
  marks.push(mark);
  for (const listener of listeners) listener(mark);
}

export function onPerfMark(listener: (mark: PerfMark) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function perfSnapshot(): PerfMark[] {
  return [...marks];
}

/** Finds the first mark matching `name`, optionally filtered by detail. */
export function findMark(
  name: string,
  detailKey?: string,
  detailValue?: unknown,
): PerfMark | undefined {
  return marks.find((mark) =>
    mark.name === name
    && (detailKey === undefined || String(mark.detail?.[detailKey]) === String(detailValue))
  );
}
