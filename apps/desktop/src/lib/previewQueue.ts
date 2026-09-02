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
 * Numeric priority used by the serial queue. Higher runs first. The frontend
 * queue is the first layer of the two-tier scheduler: it orders pending work
 * so on-screen (visible) and loupe thumbnails jump ahead of off-screen
 * (nearby) overscan work, and drops requests that scroll out of view before
 * they start.
 */
export function priorityWeight(priority: PreviewPriority): number {
  switch (priority) {
    case "loupe":
      return 2;
    case "visible":
      return 1;
    case "nearby":
      return 0;
    case "preload":
      return -1;
  }
}

export class SerialTaskQueue {
  private active = false;
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
    if (this.active) return;
    const task = this.pending.shift();
    if (!task) return;
    if (task.signal?.aborted) {
      task.reject(task.signal.reason ?? new DOMException("Aborted", "AbortError"));
      this.drain();
      return;
    }
    this.active = true;
    task.run(task.priority)
      .then(task.resolve, task.reject)
      .finally(() => {
        this.active = false;
        this.drain();
      });
  }
}

/**
 * Unified preview queue. Covers every format and every preview stage so that a
 * visible RAW/HEIF/TIFF thumbnail is never stuck behind unrelated work. The
 * backend decode gate is the second layer; this queue ensures only one preview
 * request is in flight at a time and that pending requests are both prioritized
 * and droppable via AbortSignal.
 */
export const previewQueue = new SerialTaskQueue();
