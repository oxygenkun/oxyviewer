import { useEffect } from "react";

interface BackgroundAssetPagination {
  scopeKey: string;
  enabled: boolean;
  pageCount: number;
  hasNextPage: boolean;
  isFetching: boolean;
  isError: boolean;
  fetchNextPage: (options: { cancelRefetch: false }) => Promise<unknown>;
}

/** Yield after each page so painting and thumbnail scheduling can run first. */
export function useBackgroundAssetPagination({
  scopeKey, enabled, pageCount, hasNextPage, isFetching, isError, fetchNextPage,
}: BackgroundAssetPagination): void {
  useEffect(() => {
    if (!enabled || !pageCount || !hasNextPage || isFetching || isError) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const frame = requestAnimationFrame(() => {
      timer = setTimeout(() => {
        // Scroll-driven pagination may have started since this effect ran.
        void fetchNextPage({ cancelRefetch: false }).catch(() => undefined);
      }, 200);
    });
    return () => {
      cancelAnimationFrame(frame);
      clearTimeout(timer);
    };
  }, [scopeKey, enabled, pageCount, hasNextPage, isFetching, isError, fetchNextPage]);
}
