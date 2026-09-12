import type { PreviewPriority } from "../types";

type PendingTask<T> = {
  key?: string;
  priority: number;
  signal?: AbortSignal;
  run: (effectivePriority: number) => Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
};

/**
 * Numeric priority retained for WebView-side transfer and image decode
 * preloading. Native render/decode scheduling is owned by Rust.
 */
export function priorityWeight(priority: PreviewPriority): number {
  switch (priority) {
    case "loupe":
      return 3_000_000;
    case "visible":
      return 2_000_000;
    case "nearby":
      return 1_000_000;
    case "preload":
      return 0;
  }
}

/** Preserves priority tiers while ordering equal-tier work around the selection. */
export function orderedPriorityWeight(priority: PreviewPriority, rank = 0): number {
  const boundedRank = Math.min(Math.max(0, rank), 999_999);
  return priorityWeight(priority) - boundedRank;
}

export class SerialTaskQueue {
  private active = 0;

  constructor(private readonly concurrency = 1) {}
  private pending: Array<PendingTask<unknown>> = [];

  enqueue<T>(
    priority: number,
    signal: AbortSignal | undefined,
    run: (effectivePriority: number) => Promise<T>,
    key?: string,
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      this.pending.push({
        key,
        priority,
        signal,
        run,
        resolve: resolve as (value: unknown) => void,
        reject,
      });
      this.pending.sort((left, right) => right.priority - left.priority);
      this.drain();
    });
  }

  /**
   * Raise a still-pending artifact without changing its cache identity. This
   * lets a loupe observer promote the same request a sidebar observer started.
   * Running work remains non-preemptive by design.
   */
  raisePriority(key: string, priority: number): boolean {
    const task = this.pending.find((candidate) => candidate.key === key);
    if (!task || task.priority >= priority) return false;
    task.priority = priority;
    this.pending.sort((left, right) => right.priority - left.priority);
    return true;
  }

  private drain() {
    if (this.active >= this.concurrency) return;
    const task = this.pending.shift();
    if (!task) return;
    if (task.signal?.aborted) {
      task.reject(task.signal.reason ?? new DOMException("Aborted", "AbortError"));
      this.drain();
      return;
    }
    this.active += 1;
    task.run(task.priority)
      .then(task.resolve, task.reject)
      .finally(() => {
        this.active -= 1;
        this.drain();
      });
  }
}

/**
 * Presentation-only queue for bytes that already have a Rust-owned artifact.
 * It does not decide resource validity or schedule native decode work.
 */
export const browserImageWorkerCount = Math.max(1, Math.min(4, globalThis.navigator?.hardwareConcurrency || 1));
export const browserPreloadQueue = new SerialTaskQueue(browserImageWorkerCount);
