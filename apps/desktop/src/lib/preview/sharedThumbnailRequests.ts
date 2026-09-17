import type { AssetSummary, PreviewPriority, PreviewResult } from "@/types";
import { releaseUnretainedMediaResource } from "@/lib/cache/mediaResourceLease";

type Result = PreviewResult | undefined;
type Start = (signal: AbortSignal, priority: PreviewPriority, rank: number) => Promise<Result>;

interface Consumer {
  priority: PreviewPriority;
  rank: number;
  resolve: (result: Result) => void;
  reject: (error: unknown) => void;
  unsubscribe: () => void;
}

interface Request {
  key: string;
  path: string;
  controller: AbortController;
  consumers: Set<Consumer>;
  settled: boolean;
}

const tiers: Record<PreviewPriority, number> = { loupe: 0, visible: 1, nearby: 2, preload: 3 };
const parentDirectory = (path: string) => path.replace(/[\\/][^\\/]+$/, "").toLocaleLowerCase();

/** Share only in-flight thumbnail work; projections remain the result cache. */
class SharedThumbnailRequests {
  private readonly requests = new Map<string, Request>();

  request(
    asset: AssetSummary,
    signal: AbortSignal | undefined,
    priority: PreviewPriority,
    rank: number,
    start: Start,
  ): Promise<Result> {
    if (signal?.aborted) return Promise.reject(signal.reason);
    const key = JSON.stringify([asset.path, asset.id, asset.kind, asset.modifiedAtMs, asset.sizeBytes]);
    let request = this.requests.get(key);
    const fresh = !request;
    if (!request) {
      request = { key, path: asset.path, controller: new AbortController(), consumers: new Set(),
        settled: false };
      this.requests.set(key, request);
    }
    const shared = request;
    const result = new Promise<Result>((resolve, reject) => {
      const consumer: Consumer = { priority, rank, resolve, reject, unsubscribe: () => {} };
      const abort = () => {
        consumer.unsubscribe();
        shared.consumers.delete(consumer);
        reject(signal?.reason ?? new DOMException("Aborted", "AbortError"));
        // StrictMode/query replacement may subscribe again in this same turn.
        queueMicrotask(() => {
          if (!shared.settled && shared.consumers.size === 0) this.cancel(shared);
        });
      };
      consumer.unsubscribe = () => signal?.removeEventListener("abort", abort);
      shared.consumers.add(consumer);
      signal?.addEventListener("abort", abort, { once: true });
    });
    if (fresh) {
      // Coalesce component and preloader effects before opening an IPC request.
      queueMicrotask(() => {
        if (shared.settled || !shared.consumers.size) return;
        const [first] = [...shared.consumers].sort((left, right) =>
          tiers[left.priority] - tiers[right.priority] || left.rank - right.rank);
        void start(shared.controller.signal, first.priority, first.rank).then(
          (value) => this.finish(shared, { value }),
          (error: unknown) => this.finish(shared, { error }),
        );
      });
    }
    return result;
  }

  invalidate(directory?: string): void {
    const normalized = directory?.replace(/[\\/]+$/, "").toLocaleLowerCase();
    for (const request of this.requests.values()) {
      if (normalized === undefined || parentDirectory(request.path) === normalized) this.cancel(request);
    }
  }

  private cancel(request: Request): void {
    if (request.settled) return;
    const error = new DOMException("Thumbnail request cancelled", "AbortError");
    request.controller.abort(error);
    this.finish(request, { error });
  }

  private finish(request: Request, outcome: { value: Result } | { error: unknown }): void {
    const consumers = [...request.consumers];
    request.consumers.clear();
    request.settled = true;
    if (this.requests.get(request.key) === request) this.requests.delete(request.key);
    for (const consumer of consumers) {
      consumer.unsubscribe();
      if ("error" in outcome) consumer.reject(outcome.error);
      else consumer.resolve(outcome.value);
    }
    if (!consumers.length && "value" in outcome && outcome.value?.resource) {
      releaseUnretainedMediaResource(outcome.value.resource.resourceId);
    }
  }
}

export const sharedThumbnailRequests = new SharedThumbnailRequests();
