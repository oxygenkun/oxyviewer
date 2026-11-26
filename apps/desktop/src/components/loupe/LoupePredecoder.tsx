import { useCallback, useEffect, useState } from "react";
import { predecodeFullImage } from "@/lib/preview/fullPredecode";
import { isTauri } from "@/lib/api";
import type { AssetSummary, HeifDecodeStatus } from "@/types";
import { HeifTileCanvas } from "./HeifTileCanvas";

const ignoreSize = () => {};
const identity = (asset: AssetSummary) => `${asset.path}\0${asset.modifiedAtMs}`;

/** One speculative full image at a time; the current image opens the gate. */
export function LoupePredecoder({ assets, ready, displaySharpening }: {
  assets: AssetSummary[];
  ready: boolean;
  displaySharpening: boolean;
}) {
  const [completed, setCompleted] = useState<Set<string>>(() => new Set());
  const next = ready ? assets.find((asset) => !completed.has(identity(asset))) : undefined;
  const finish = useCallback(() => {
    if (next) setCompleted((previous) => new Set([...previous, identity(next)]));
  }, [next]);
  const heifStatus = useCallback((status: HeifDecodeStatus) => {
    if (status === "failed" || status === "cancelled") finish();
  }, [finish]);
  const tileDelivery = next?.kind === "heif" && isTauri();
  // A changed window/setting cancels work immediately. Completed candidates
  // remain remembered when more directory pages arrive.
  const key = next ? identity(next) : undefined;
  const candidate = next;
  useEffect(() => {
    if (!candidate || tileDelivery) return;
    const controller = new AbortController();
    void predecodeFullImage(candidate, controller.signal)
      .catch(() => undefined)
      .finally(() => { if (!controller.signal.aborted) finish(); });
    return () => controller.abort();
  }, [candidate, tileDelivery, finish]);
  if (!next || !tileDelivery) return null;
  return <div aria-hidden="true" style={{ position: "absolute", width: 0, height: 0, overflow: "hidden", pointerEvents: "none" }}>
    <HeifTileCanvas key={key} asset={next} displaySharpening={displaySharpening}
      priority="preload" onImageSize={ignoreSize} onStatus={heifStatus} onPresentationReady={finish} />
  </div>;
}
