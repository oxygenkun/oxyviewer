import { useQuery } from "@tanstack/react-query";
import { useEffect, useState, useRef } from "react";
import { createPersonInstance, listPersonInstances, setPersonReview, updatePersonInstance } from "@/lib/api";
import type { AssetSummary } from "@/types";
import { usePeopleWorkspace, type Box } from "./PeopleContext";
export function PersonBoxOverlay({ asset }: { asset: AssetSummary }) {
  const workspace = usePeopleWorkspace();
  const scope = useRef(workspace);
  scope.current = workspace;
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const [draft, setDraft] = useState<Box>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const start = useRef<[number, number] | undefined>(undefined);
  const instances = useQuery({ queryKey: ["person-instances", workspace?.folderPath, asset.path], queryFn: () => listPersonInstances(workspace!.folderPath, asset.path), enabled: Boolean(workspace) });
  if (!workspace || !workspace.showBoxes) return null;
  const point = (event: React.PointerEvent<HTMLDivElement>): [number, number] => {
    const rect = event.currentTarget.getBoundingClientRect();
    return [Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)), Math.max(0, Math.min(1, (event.clientY - rect.top) / rect.height))];
  };
  const save = async (box: Box) => {
    setBusy(true); setError(undefined);
    try {
      const edit = workspace.editInstance?.assetPath === asset.path ? workspace.editInstance : undefined;
      const face = workspace.draw === "face" ? box : edit?.faceBox ?? null;
      const body = workspace.draw === "body" ? box : edit?.bodyBox ?? null;
      const instance = edit ? await updatePersonInstance(edit, asset, face, body) : await createPersonInstance(workspace.folderPath, asset, face, body);
      if (!edit) await setPersonReview(workspace.folderPath, instance.id, workspace.person.id, "pending", 0);
      if (alive.current && scope.current?.person.id === workspace.person.id && scope.current?.folderPath === workspace.folderPath) { workspace.selectInstance(instance.id); workspace.setDraw(undefined); workspace.setEditInstance(undefined); }
      await workspace.changed(asset);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); setDraft(undefined); }
  };
  return <div className={`person-box-overlay ${workspace.draw ? "is-drawing" : ""}`} aria-label="人物框画布"
    onPointerDown={event => { if (!workspace.draw || busy || event.button !== 0) return; event.stopPropagation(); event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); start.current = point(event); }}
    onPointerMove={event => { if (!start.current) return; event.stopPropagation(); const [x,y] = point(event); const [sx,sy] = start.current; setDraft([Math.min(x,sx),Math.min(y,sy),Math.abs(x-sx),Math.abs(y-sy)]); }}
    onPointerUp={event => { if (!start.current) return; event.stopPropagation(); const [x,y] = point(event); const [sx,sy] = start.current; start.current = undefined; const box: Box = [Math.min(x,sx),Math.min(y,sy),Math.abs(x-sx),Math.abs(y-sy)]; if (box[2] > .005 && box[3] > .005) void save(box); else setDraft(undefined); }}
    onPointerCancel={() => { start.current = undefined; setDraft(undefined); }}>
    {(instances.data ?? []).flatMap((instance, index) => ([['face',instance.faceBox],['body',instance.bodyBox]] as const).map(([kind,box]) => box ? <button key={`${instance.id}:${kind}`} type="button" aria-label={`选择人物实例 ${index + 1} ${kind === 'face' ? '人脸' : '人体'}`} className={`person-image-box ${workspace.selectedInstanceId === instance.id ? 'is-selected' : ''} ${instance.sourceRevision !== `${asset.sizeBytes}:${asset.modifiedAtMs}` ? 'is-stale' : ''}`}
      style={{left:`${box[0]*100}%`,top:`${box[1]*100}%`,width:`${box[2]*100}%`,height:`${box[3]*100}%`}}
      onPointerDown={event => { if (!workspace.draw) event.stopPropagation(); }} onClick={event => { event.stopPropagation(); workspace.selectInstance(instance.id); }}><span>{index+1} · {kind === 'face' ? '脸' : '人体'}</span></button> : null))}
    {draft ? <div className="person-image-box is-draft" style={{left:`${draft[0]*100}%`,top:`${draft[1]*100}%`,width:`${draft[2]*100}%`,height:`${draft[3]*100}%`}} /> : null}
    {error ? <p role="alert" className="person-overlay-error">{error}</p> : null}
  </div>;
}

