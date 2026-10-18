import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import {
  chooseAndConfigureExiftool,
  cancelMetadataSync,
  getExiftoolStatus,
  installExiftool,
  listAssets,
  onEmbeddedSyncProgress,
  syncEmbeddedMetadataBatch,
} from "@/lib/api";
import { firstBrowseCursor, nextBrowseCursor, type BrowseCursor } from "@/lib/browse/browsePagination";
import type { MessageKey } from "@/lib/i18n";
import type { ExiftoolSetupAction } from "@/lib/projection/metadataProvider";
import type { FolderSession } from "@/types";

export interface FolderMetadataSyncState {
  directory: string;
  phase: "scanning" | "setup" | "writing" | "done";
  total: number;
  completed: number;
  succeeded: number;
  unchanged: number;
  failureCount: number;
  skipped: number;
  failures: Array<{ path: string; error: string; uncertain: boolean }>;
  cancelled: boolean;
  settingUp: boolean;
  setupAction?: ExiftoolSetupAction;
  error?: string;
}

interface SyncRun {
  paths: string[];
  cancelled: boolean;
  busy: boolean;
  skipped: number;
  nativeFinished?: boolean;
  jobId?: string;
  unlisten?: () => void;
}

/** Folder writes start only from an explicit menu action, independently of selection/filter state. */
export function useFolderMetadataSync(t: (key: MessageKey) => string) {
  const queryClient = useQueryClient();
  const [state, setState] = useState<FolderMetadataSyncState>();
  const current = useRef<SyncRun | undefined>(undefined);
  useEffect(() => () => {
    if (current.current) {
      const run = current.current;
      run.cancelled = true;
      if (run.jobId) { void cancelMetadataSync(run.jobId).catch(() => {}); run.unlisten?.(); }
    }
    current.current = undefined;
  }, []);

  function update(run: SyncRun, patch: Partial<FolderMetadataSyncState>) {
    if (current.current === run) setState(previous => previous ? { ...previous, ...patch } : previous);
  }

  function finish(run: SyncRun, error?: string) {
    run.busy = false;
    update(run, { phase: "done", cancelled: run.cancelled, settingUp: false, error });
  }

  async function write(run: SyncRun) {
    run.busy = true;
    update(run, { phase: "writing", settingUp: false, error: undefined });
    const requestId = crypto.randomUUID();
    let needsRefresh = false;
    run.unlisten = await onEmbeddedSyncProgress(progress => {
      if (progress.requestId !== requestId || run.nativeFinished) return;
      run.jobId = progress.jobId;
      needsRefresh ||= progress.updated > 0;
      if (run.cancelled) {
        void cancelMetadataSync(progress.jobId).catch(cause => update(run, { error: String(cause) }));
        if (current.current !== run) run.unlisten?.();
      }
      update(run, { completed: progress.completed, succeeded: progress.updated, unchanged: progress.unchanged,
        skipped: run.skipped + progress.skipped,
        failureCount: progress.completed - progress.updated - progress.unchanged - progress.skipped });
    });
    try {
      if (run.cancelled) return finish(run);
      // One command owns the complete batch; progress is subscribed before admission.
      const result = await syncEmbeddedMetadataBatch(run.paths, requestId);
      run.nativeFinished = true;
      needsRefresh ||= result.updated > 0 || result.failures.some(failure => failure.uncertain);
      run.cancelled = result.cancelled;
      update(run, { total: result.total, completed: result.completed, succeeded: result.updated,
        unchanged: result.unchanged, failureCount: result.failures.length, failures: result.failures });
      update(run, { skipped: run.skipped + result.skipped });
      finish(run, !result.cancelled && result.completed < result.total ? t("folderMetadataSyncInterrupted") : undefined);
    } catch (cause) {
      run.nativeFinished = true;
      // A lost reply can hide partial writes; refresh without repeating the command.
      needsRefresh = true;
      finish(run, String(cause));
    } finally {
      run.unlisten?.();
      if (needsRefresh) await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["asset-details"] }),
        queryClient.invalidateQueries({ queryKey: ["assets"] }),
        queryClient.invalidateQueries({ queryKey: ["preload-assets"] }),
        queryClient.invalidateQueries({ queryKey: ["progressive-metadata-assets"] }),
      ]);
    }
  }

  async function start(session: FolderSession, directory: string) {
    if (current.current) return;
    const run: SyncRun = { paths: [], cancelled: false, busy: true, skipped: 0 };
    current.current = run;
    setState({ directory, phase: "scanning", total: 0, completed: 0, succeeded: 0,
      unchanged: 0, failureCount: 0, skipped: 0, failures: [], cancelled: false, settingUp: false });
    try {
      let cursor: BrowseCursor | undefined = firstBrowseCursor;
      let skipped = 0;
      const seen = new Set<string>();
      while (cursor && !run.cancelled) {
        const page = await listAssets(session.id, directory,
          { sort: "name", direction: "ascending", pageSize: 250 }, cursor.offset, cursor.snapshotRevision);
        if (run.cancelled) break;
        if (page.progress?.stage === "stale") throw new Error(t("folderMetadataSyncStale"));
        for (const asset of page.items) {
          if (seen.has(asset.path)) continue;
          seen.add(asset.path);
          if (asset.hasSidecar && (asset.kind === "jpeg" || asset.kind === "heif")) run.paths.push(asset.path);
          else skipped += 1;
        }
        update(run, { total: run.paths.length, skipped });
        run.skipped = skipped;
        const next = nextBrowseCursor(page);
        if (next && next.offset <= cursor.offset) throw new Error(t("folderMetadataSyncStale"));
        cursor = next;
        // Give progress, cancellation and painting a turn between pages, even
        // when browser-demo or cached requests resolve immediately.
        if (cursor) await new Promise<void>(resolve => window.setTimeout(resolve, 0));
      }
      if (run.cancelled || !run.paths.length) return finish(run);
      const status = await getExiftoolStatus();
      if (run.cancelled) return finish(run);
      if (!status.available) {
        run.busy = false;
        update(run, { phase: "setup" });
        return;
      }
      await write(run);
    } catch (cause) {
      finish(run, String(cause));
    }
  }

  async function setup(source: ExiftoolSetupAction) {
    const run = current.current;
    if (!run || run.busy || run.cancelled) return;
    run.busy = true;
    update(run, { settingUp: true, setupAction: source, error: undefined });
    try {
      const status = await (source === "check" ? getExiftoolStatus()
        : source === "download" ? installExiftool() : chooseAndConfigureExiftool());
      if (run.cancelled) return finish(run);
      if (!status?.available) {
        run.busy = false;
        update(run, { settingUp: false, error: source === "check"
          ? t("exiftoolNotFound") : status?.detail ?? undefined });
        return;
      }
      await write(run);
    } catch (cause) {
      run.busy = false;
      if (run.cancelled) finish(run);
      else update(run, { settingUp: false, error: String(cause) });
    }
  }

  function cancel() {
    const run = current.current;
    if (!run) return;
    run.cancelled = true;
    update(run, { cancelled: true });
    if (run.jobId) void cancelMetadataSync(run.jobId).catch(cause => update(run, { error: String(cause) }));
    if (!run.busy) finish(run);
  }

  function close() {
    if (current.current?.busy) return cancel();
    if (current.current) current.current.cancelled = true;
    current.current = undefined;
    setState(undefined);
  }

  return { state, start, setup, cancel, close };
}
