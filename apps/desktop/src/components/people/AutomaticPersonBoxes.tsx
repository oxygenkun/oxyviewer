import { useContext, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getPersonDetections, listPersonInstances } from "@/lib/api";
import { PersonDetectionContext, usePeopleWorkspace } from "./PeopleContext";
import type { AssetSummary } from "@/types";

export function AutomaticPersonBoxes({ asset }: { asset: AssetSummary }) {
  const context = useContext(PersonDetectionContext);
  const manual = usePeopleWorkspace();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const detections = useQuery({ queryKey: ["person-detections", context?.folderPath, asset.path, asset.sizeBytes, asset.modifiedAtMs], queryFn: () => getPersonDetections(context!.folderPath, asset.path), enabled: Boolean(context?.showBoxes) });
  const instances = useQuery({ queryKey: ["person-instances", context?.folderPath, asset.path], queryFn: () => listPersonInstances(context!.folderPath, asset.path), enabled: Boolean(context?.showBoxes) });
  if (!context?.showBoxes || manual?.draw) return null;
  return <div className="person-box-overlay" aria-label="自动检测人脸">
    {detections.data?.instances.map((instance, index) => {
      const box = instance.faceBox;
      if (!box || instances.data?.some(item => JSON.stringify(item.faceBox) === JSON.stringify(box))) return null;
      return <button key={instance.instanceId} className="person-image-box is-stale" type="button" disabled={busy}
        aria-label={`采用检测人脸 ${index + 1}`} title="采用此框，加入人工审阅（不会自动确认身份）"
        style={{ left: `${box[0] * 100}%`, top: `${box[1] * 100}%`, width: `${box[2] * 100}%`, height: `${box[3] * 100}%`, borderStyle: "dashed" }}
        onPointerDown={event => event.stopPropagation()}
        onClick={event => { event.stopPropagation(); setBusy(true); setError(undefined); void context.adopt(asset, detections.data!.sourceRevision, instance.instanceId).catch(cause => setError(String(cause))).finally(() => setBusy(false)); }}>
        <span>检测 {index + 1} · 待核对</span>
      </button>;
    })}
    {error || detections.error ? <p role="alert" className="person-overlay-error">{error ?? String(detections.error)}</p> : null}
  </div>;
}
