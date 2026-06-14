type PendingTask<T> = {
  priority: number;
  signal?: AbortSignal;
  run: () => Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
};

export class SerialTaskQueue {
  private active = false;
  private pending: Array<PendingTask<unknown>> = [];

  enqueue<T>(priority: number, signal: AbortSignal | undefined, run: () => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      this.pending.push({
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
    task.run()
      .then(task.resolve, task.reject)
      .finally(() => {
        this.active = false;
        this.drain();
      });
  }
}

export const heifThumbnailQueue = new SerialTaskQueue();
