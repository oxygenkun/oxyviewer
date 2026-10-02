import { useRef, useState } from "react";
import type { AssetSummary, PersonBox } from "@/types";
import { savePersonTupleGeometry } from "@/lib/api";
import { useGlobalPeopleContext, uniquePersonTuples } from "./GlobalPeopleContext";

export function GlobalPersonBoxes({ asset }: { asset: AssetSummary }) {
  const state = useGlobalPeopleContext(); const scope = useRef(state); scope.current = state;
  const start = useRef<[number, number] | undefined>(undefined); const [draft, setDraft] = useState<PersonBox>(); const [busy, setBusy] = useState(false); const [error, setError] = useState("");
  if (!state?.enabled || !state.folderPath || !state.showBoxes) return null;
  const tuples = uniquePersonTuples((state.workspace?.groups.flatMap(g => g.members) ?? []).filter(t => t.assetPath === asset.path));
  const point = (e: React.PointerEvent<HTMLDivElement>): [number, number] => { const r = e.currentTarget.getBoundingClientRect(); return [Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)), Math.max(0, Math.min(1, (e.clientY - r.top) / r.height))]; };
  const save = async (box: PersonBox) => {
    setBusy(true); setError(""); const origin = state.folderPath!; const edit = state.edit?.assetPath === asset.path ? state.edit : undefined;
    try {
      const tuple = await savePersonTupleGeometry(origin, asset, state.draw === "face" ? box : edit?.faceBox ?? null, state.draw === "body" ? box : edit?.bodyBox ?? null, edit); await state.refresh();
      if (scope.current?.folderPath === origin) { state.setDraw(undefined); state.setEdit(undefined); if (!edit) state.setFilter({ groupId: `manual:${tuple.id}` }); state.choose(tuple.id); }
    } catch (e) { setError(String(e)); } finally { setBusy(false); setDraft(undefined); }
  };
  return <div className={`person-box-overlay ${state.draw ? "is-drawing" : ""}`} aria-label="人物实例画布"
    onPointerDown={e => { if (!state.draw || busy || e.button !== 0) return; e.preventDefault(); e.stopPropagation(); e.currentTarget.setPointerCapture(e.pointerId); start.current = point(e); }}
    onPointerMove={e => { if (!start.current) return; e.stopPropagation(); const [x, y] = point(e), [sx, sy] = start.current; setDraft([Math.min(x, sx), Math.min(y, sy), Math.abs(x - sx), Math.abs(y - sy)]); }}
    onPointerUp={e => { if (!start.current) return; e.stopPropagation(); const [x, y] = point(e), [sx, sy] = start.current; start.current = undefined; const box: PersonBox = [Math.min(x, sx), Math.min(y, sy), Math.abs(x - sx), Math.abs(y - sy)]; if (box[2] > .005 && box[3] > .005) void save(box); else setDraft(undefined); }}
    onPointerCancel={() => { start.current = undefined; setDraft(undefined); }}>
    {tuples.flatMap((t, i) => ([["面部", t.faceBox], ["人体", t.bodyBox]] as const).map(([kind, box]) => box ? <button key={`${t.id}:${kind}`} type="button" disabled={busy || !!state.draw} aria-label={`选择实例 ${i + 1} ${kind}`} className={`person-image-box ${t.decision === "belongs" ? "is-confirmed" : ""} ${state.selected.has(t.id) ? "is-selected" : ""} ${t.needsReview || t.sourceRevision !== `${asset.sizeBytes}:${asset.modifiedAtMs}` ? "is-stale" : ""}`} style={{ left: `${box[0] * 100}%`, top: `${box[1] * 100}%`, width: `${box[2] * 100}%`, height: `${box[3] * 100}%` }} onPointerDown={e => e.stopPropagation()} onClick={e => { e.stopPropagation(); state.choose(t.id, e.ctrlKey || e.metaKey, e.shiftKey); }}><span>{i + 1} · {state.people.find(p => p.id === t.personId)?.displayName ?? `未知实例 ${i + 1}`} · {kind}</span></button> : null))}
    {draft ? <div className="person-image-box is-draft" style={{ left: `${draft[0] * 100}%`, top: `${draft[1] * 100}%`, width: `${draft[2] * 100}%`, height: `${draft[3] * 100}%` }} /> : null}
    {error ? <p role="alert" className="person-overlay-error">{error}</p> : null}
  </div>;
}
