import { runResourceStress } from "@/lib/diagnostics/resourceStress";
import { useEffect, useRef } from "react";
import { writePerfReport } from "@/lib/api";
import {
  activatePerfProbe,
  onPerfMark,
  perfMark,
  perfSnapshot,
} from "@/lib/diagnostics/perfProbe";
import { useWorkspaceStore } from "@/store";
import { getFolderThumbnailStats } from "@/lib/cache/folderThumbnailCache";
import type { AssetSummary, FolderSession, PerfScenario } from "@/types";

interface PerfHarnessProps {
  scenario: PerfScenario;
  session?: FolderSession;
  assets: AssetSummary[];
  assetsLoading: boolean;
  onOpenPath: (path: string) => Promise<void>;
}

const DEFAULT_TIMEOUT_MS = 30_000;

/**
 * Await token syntax: `mark-name`, `mark-name@level` (shorthand for the
 * semantic detail.stage), or `mark-name@key=value` matched on any detail field.
 * Image marks are restricted to the selected asset's large loupe image.
 */
function markSatisfied(token: string, scenario: PerfScenario): boolean {
  const atIndex = token.indexOf("@");
  const name = atIndex === -1 ? token : token.slice(0, atIndex);
  const condition = atIndex === -1 ? undefined : token.slice(atIndex + 1);
  const equalsIndex = condition?.indexOf("=") ?? -1;
  const key = condition === undefined
    ? undefined
    : equalsIndex === -1
      ? "stage"
      : condition.slice(0, equalsIndex);
  const value = condition === undefined
    ? undefined
    : equalsIndex === -1
      ? condition
      : condition.slice(equalsIndex + 1);
  return perfSnapshot().some((mark) =>
    mark.name === name
    && (!scenario.selectName || mark.detail?.assetName === scenario.selectName)
    && (name !== "image:loaded" || mark.detail?.large === !scenario.scrollToEnd)
    && (key === undefined || String(mark.detail?.[key]) === String(value))
  );
}

/**
 * Drives one automated performance scenario through the real UI path
 * (open folder → first page → select → loupe) and writes a JSON mark report
 * for tests/perf/perf-e2e.mjs. Renders nothing; see docs/PERF_E2E.md.
 */
export function PerfHarness({
  scenario,
  session,
  assets,
  assetsLoading,
  onOpenPath,
}: PerfHarnessProps) {
  const assetsRef = useRef(assets);
  assetsRef.current = assets;
  const stressRef = useRef(false);
  const stressAbort = useRef<AbortController | undefined>(undefined);
  const openedRef = useRef(false);
  const firstPageRef = useRef(false);
  const selectedRef = useRef(false);
  const doneRef = useRef(false);

  useEffect(() => {
    // Read-only browser probe, exposed only by an explicitly injected scenario.
    Object.defineProperty(window, "__oxyPerfInspect", {
      configurable: true,
      value: () => ({ marks: perfSnapshot(), retained: getFolderThumbnailStats() }),
    });
    return () => { Reflect.deleteProperty(window, "__oxyPerfInspect"); };
  }, []);

  useEffect(() => {
    activatePerfProbe();
    if (openedRef.current) return;
    openedRef.current = true;
    perfMark("harness:start", { scenario: scenario.name });
    void onOpenPath(scenario.folder);
  }, [onOpenPath, scenario]);

  useEffect(() => {
    if (firstPageRef.current || doneRef.current || !session || assetsLoading) return;
    let cancelled = false;
    // Double rAF: one frame for React commit, one for the browser paint. The
    // effect re-runs if assetsLoading flips again, so only record firstPageRef
    // once the paint mark actually fires.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        if (cancelled || doneRef.current) return;
        firstPageRef.current = true;
        perfMark("harness:first-page-painted");
        if (scenario.scrollToEnd) {
          const scroller = document.querySelector<HTMLElement>(".asset-scroll");
          if (scroller) {
            perfMark("harness:viewport-jump", { assetName: scenario.selectName });
            scroller.scrollTop = scroller.scrollHeight;
            scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
          }
        }
      })
    );
    return () => {
      cancelled = true;
    };
  }, [assetsLoading, scenario.scrollToEnd, scenario.selectName, session]);

  useEffect(() => {
    if (
      selectedRef.current
      || doneRef.current
      || Boolean(scenario.resourceStress)
      || !scenario.selectName
      || scenario.scrollToEnd
      || !session
      || assetsLoading
    ) return;
    const asset = assets.find((item) => item.name === scenario.selectName);
    if (!asset) return;
    selectedRef.current = true;
    const { select, setView } = useWorkspaceStore.getState();
    perfMark("harness:select", { assetName: asset.name });
    select(asset.id);
    if (scenario.enterLoupe !== false) setView("loupe");
  }, [assets, assetsLoading, scenario, session]);

  useEffect(() => {
    const controller = new AbortController();
    stressAbort.current = controller;
    return () => controller.abort();
  }, []);

  useEffect(() => {
    if (!scenario.resourceStress || !session || assetsLoading || stressRef.current) return;
    const controller = stressAbort.current!;
    // StrictMode may clean up the initial effect before any work starts.
    queueMicrotask(() => {
      if (controller.signal.aborted) return;
      stressRef.current = true;
      void runResourceStress(scenario.resourceStress!, () => assetsRef.current, controller.signal, scenario.expectedAssets)
        .catch((error) => {
          if (!controller.signal.aborted) perfMark("resource:stress-failed", { message: String(error) });
        });
    });
  }, [scenario, session?.id, assetsLoading]);

  useEffect(() => {
    const finalize = (reason: "complete" | "timeout" | "failed") => {
      if (doneRef.current) return;
      doneRef.current = true;
      stressAbort.current?.abort();
      perfMark("harness:done", { reason });
      void writePerfReport(scenario.reportPath, {
        scenario: scenario.name,
        generatedAt: new Date().toISOString(),
        userAgent: navigator.userAgent,
        marks: perfSnapshot(),
      });
    };
    const check = () => {
      if (perfSnapshot().some((mark) => mark.name === "resource:stress-failed")) {
        finalize("failed");
        return;
      }
      if (scenario.awaitMarks.every((token) => markSatisfied(token, scenario))) finalize("complete");
    };
    const unsubscribe = onPerfMark(check);
    check();
    const timer = setTimeout(() => finalize("timeout"), scenario.timeoutMs ?? DEFAULT_TIMEOUT_MS);
    return () => {
      unsubscribe();
      clearTimeout(timer);
    };
  }, [scenario]);

  return null;
}
