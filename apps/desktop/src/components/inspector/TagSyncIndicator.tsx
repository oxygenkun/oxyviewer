import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw, TriangleAlert } from "lucide-react";
import { useEffect } from "react";
import { getTagSyncStatus, onTagSyncStatusChanged, retryTagXmpSync } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";

/** The queue reports every change; this only repairs a listener that missed one. */
const FALLBACK_POLL_MS = 15_000;

/**
 * Mirror-queue state for the whole window, beside the inspector's first section.
 *
 * It never participates in the inspector body's flow: mounting, hiding or
 * resolving it cannot change the panel height, so a tag edit no longer
 * crosses the scroll threshold for a moment and flashes a scrollbar.
 */
export function TagSyncIndicator({ t }: { t: (key: MessageKey) => string }) {
  const queryClient = useQueryClient();
  const statusQuery = useQuery({
    queryKey: ["tag-sync-status"],
    queryFn: getTagSyncStatus,
    refetchInterval: (query) => query.state.data?.pendingCount ? FALLBACK_POLL_MS : false,
  });
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onTagSyncStatusChanged((status) => {
      queryClient.setQueryData(["tag-sync-status"], status);
    }).then((dispose) => { if (disposed) dispose(); else unlisten = dispose; });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queryClient]);
  const retryMutation = useMutation({
    mutationFn: retryTagXmpSync,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tag-sync-status"] }),
  });

  const failed = Boolean(statusQuery.data?.failedCount);
  const pending = Boolean(statusQuery.data?.pendingCount);
  const state = failed ? "error" : pending ? "pending" : "idle";
  const label = failed ? t("tagSyncFailed") : t("tagSyncPending");
  return (
    <button
      type="button"
      className="tag-sync-indicator"
      data-state={state}
      title={state === "idle" ? undefined : failed ? `${label} · ${t("retry")}` : label}
      aria-label={state === "idle" ? undefined : label}
      aria-hidden={state === "idle"}
      aria-disabled={state !== "error"}
      tabIndex={state === "error" ? 0 : -1}
      onClick={() => { if (state === "error") retryMutation.mutate(); }}
    >
      {failed ? <TriangleAlert size={12} /> : <RefreshCw size={12} />}
    </button>
  );
}
