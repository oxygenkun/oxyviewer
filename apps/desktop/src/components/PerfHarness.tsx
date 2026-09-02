import { useEffect, useRef } from "react";
import { writePerfReport } from "../lib/api";
import {
  activatePerfProbe,
  onPerfMark,
  perfMark,
  perfSnapshot,
} from "../lib/perfProbe";
import { useWorkspaceStore } from "../store";
import type { AssetSummary, FolderSession, PerfScenario } from "../types";

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
    && (name !== "image:loaded" || mark.detail?.large === true)
    && (key === undefined || String(mark.detail?.[key]) === String(value))
  );
}

/**
 * Drives one automated performance scenario through the real UI path
 * (open folder → first page → select → loupe) and writes a JSON mark report
 * for scripts/perf-e2e.mjs. Renders nothing; see docs/PERF_E2E.md.
 */
export function PerfHarness({
  scenario,
  session,
  assets,
  assetsLoading,
  onOpenPath,
}: PerfHarnessProps) {
  const openedRef = useRef(false);
  const firstPageRef = useRef(false);
  const selectedRef = useRef(false);
  const doneRef = useRef(false);

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
      })
    );
    return () => {
      cancelled = true;
    };
  }, [session, assetsLoading]);

  useEffect(() => {
    if (
      selectedRef.current
      || doneRef.current
      || !scenario.selectName
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
    const finalize = (reason: "complete" | "timeout") => {
      if (doneRef.current) return;
      doneRef.current = true;
      perfMark("harness:done", { reason });
      void writePerfReport(scenario.reportPath, {
        scenario: scenario.name,
        generatedAt: new Date().toISOString(),
        userAgent: navigator.userAgent,
        marks: perfSnapshot(),
      });
    };
    const check = () => {
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
