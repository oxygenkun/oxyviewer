import type { PreviewPriority } from "../types";

interface PreviewDebugRequest {
  assetName: string;
  stage: string;
  priority: PreviewPriority;
  /** Stable identity of the underlying work, independent of its consumers. */
  resourceKey?: string;
  /** Short, human-readable artifact level shown by the diagnostics UI. */
  resourceLabel?: string;
}

export interface PreviewDebugHandle {
  start: () => void;
  mark: (milestone: string, detail?: Record<string, unknown>) => void;
  complete: (detail?: Record<string, unknown>) => void;
  fail: (error?: unknown, detail?: Record<string, unknown>) => void;
  cancel: (detail?: Record<string, unknown>) => void;
  updatePriority: (priority: PreviewPriority) => void;
}

export type TrackedPreview = PreviewDebugRequest & {
  id: number;
  queuedAt: number;
  startedAt?: number;
  consumers?: number;
};

export interface PreviewDebugSnapshot {
  waiting: TrackedPreview[];
  loading: TrackedPreview[];
}

const DEBUG_STORAGE_KEY = "oxyviewer.previewDebugSnapshot";

const waiting = new Map<number, TrackedPreview>();
const loading = new Map<number, TrackedPreview>();
let nextId = 1;

function now() {
  return performance.now();
}

function elapsed(startedAt: number, endedAt: number) {
  return `${(endedAt - startedAt).toFixed(1)}ms`;
}

function label(request: TrackedPreview) {
  return `${request.assetName} | ${request.stage} | ${request.priority}`;
}

function names(requests: Iterable<TrackedPreview>) {
  return [...requests].map((request) => request.assetName);
}

function printState() {
  const active = [...loading.values()];
  const visible = active.filter((request) => request.priority === "visible" || request.priority === "loupe");
  console.debug("[OxyPreview][STATE]", {
    waiting: names(waiting.values()),
    loading: names(active),
    visibleLoading: names(visible),
  });
  if (typeof window !== "undefined") {
    window.localStorage.setItem(DEBUG_STORAGE_KEY, JSON.stringify(getLocalSnapshot()));
  }
}

function getLocalSnapshot(): PreviewDebugSnapshot {
  return {
    waiting: [...waiting.values()].map((request) => ({ ...request })),
    loading: [...loading.values()].map((request) => ({ ...request })),
  };
}

export function getPreviewDebugSnapshot(): PreviewDebugSnapshot {
  if (!__OXY_DEBUG__) return { waiting: [], loading: [] };
  const local = getLocalSnapshot();
  if (local.waiting.length || local.loading.length) return local;
  if (typeof window === "undefined") return local;
  try {
    const stored = window.localStorage.getItem(DEBUG_STORAGE_KEY);
    return stored ? JSON.parse(stored) as PreviewDebugSnapshot : local;
  } catch {
    return local;
  }
}

const PRIORITY_WEIGHT: Record<PreviewPriority, number> = {
  preload: 0,
  nearby: 1,
  visible: 2,
  loupe: 3,
};

/**
 * Collapses several WebView consumers of one resource into a single row. A
 * resource is considered loading as soon as any of its consumers has started.
 */
export function aggregatePreviewDebugSnapshot(
  snapshot: PreviewDebugSnapshot,
): PreviewDebugSnapshot {
  const groups = new Map<string, TrackedPreview>();

  for (const request of [...snapshot.waiting, ...snapshot.loading]) {
    // Old persisted snapshots have no resource key, so retain their original
    // one-row-per-request behavior rather than guessing from the filename.
    const key = request.resourceKey ?? `request:${request.id}`;
    const current = groups.get(key);
    if (!current) {
      groups.set(key, { ...request, consumers: 1 });
      continue;
    }
    current.consumers = (current.consumers ?? 1) + 1;
    current.queuedAt = Math.min(current.queuedAt, request.queuedAt);
    if (request.startedAt !== undefined) {
      current.startedAt = current.startedAt === undefined
        ? request.startedAt
        : Math.min(current.startedAt, request.startedAt);
    }
    if (PRIORITY_WEIGHT[request.priority] > PRIORITY_WEIGHT[current.priority]) {
      current.priority = request.priority;
    }
  }

  const requests = [...groups.values()];
  return {
    waiting: requests.filter((request) => request.startedAt === undefined),
    loading: requests.filter((request) => request.startedAt !== undefined),
  };
}

/**
 * Tracks one preview operation in development builds. The production branch is
 * reduced to a cheap guard by Vite and does not retain per-image debug state.
 */
export function beginPreviewDebug(request: PreviewDebugRequest): PreviewDebugHandle | undefined {
  if (!__OXY_DEBUG__) return undefined;

  const tracked: TrackedPreview = {
    ...request,
    id: nextId,
    queuedAt: now(),
  };
  nextId += 1;
  waiting.set(tracked.id, tracked);
  console.debug(`[OxyPreview][WAIT] ${label(tracked)}`);
  printState();

  const finish = (
    status: "DONE" | "ERROR" | "CANCEL",
    detail?: Record<string, unknown>,
  ) => {
    const endedAt = now();
    const wasTracked = waiting.delete(tracked.id) || loading.delete(tracked.id);
    if (!wasTracked) return;
    const waitTime = elapsed(tracked.queuedAt, tracked.startedAt ?? endedAt);
    const loadTime = tracked.startedAt ? elapsed(tracked.startedAt, endedAt) : "not-started";
    const totalTime = elapsed(tracked.queuedAt, endedAt);
    console.debug(
      `[OxyPreview][${status}] ${label(tracked)} | wait=${waitTime} | load=${loadTime} | total=${totalTime}`,
      detail,
    );
    printState();
  };

  return {
    start: () => {
      if (!waiting.delete(tracked.id)) return;
      tracked.startedAt = now();
      loading.set(tracked.id, tracked);
      console.debug(
        `[OxyPreview][START] ${label(tracked)} | waited=${elapsed(tracked.queuedAt, tracked.startedAt)}`,
      );
      printState();
    },
    mark: (milestone, detail) => {
      if (!waiting.has(tracked.id) && !loading.has(tracked.id)) return;
      const markedAt = now();
      const loadTime = tracked.startedAt
        ? elapsed(tracked.startedAt, markedAt)
        : "not-started";
      console.debug(
        `[OxyPreview][MARK] ${label(tracked)} | ${milestone} | total=${elapsed(tracked.queuedAt, markedAt)} | load=${loadTime}`,
        detail,
      );
    },
    complete: (detail) => finish("DONE", detail),
    fail: (error, detail) => finish("ERROR", { ...detail, error }),
    cancel: (detail) => finish("CANCEL", detail),
    updatePriority: (priority) => {
      if (tracked.priority === priority) return;
      tracked.priority = priority;
      if (waiting.has(tracked.id) || loading.has(tracked.id)) printState();
    },
  };
}
