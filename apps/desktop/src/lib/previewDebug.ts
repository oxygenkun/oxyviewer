import type { PreviewPriority } from "../types";

interface PreviewDebugRequest {
  assetName: string;
  stage: string;
  priority: PreviewPriority;
}

export interface PreviewDebugHandle {
  start: () => void;
  mark: (milestone: string, detail?: Record<string, unknown>) => void;
  complete: (detail?: Record<string, unknown>) => void;
  fail: (error?: unknown, detail?: Record<string, unknown>) => void;
  cancel: (detail?: Record<string, unknown>) => void;
  updatePriority: (priority: PreviewPriority) => void;
}

type TrackedPreview = PreviewDebugRequest & {
  id: number;
  queuedAt: number;
  startedAt?: number;
};

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
  const visible = active.filter((request) => request.priority !== "nearby");
  console.debug("[OxyPreview][STATE]", {
    waiting: names(waiting.values()),
    loading: names(active),
    visibleLoading: names(visible),
  });
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
