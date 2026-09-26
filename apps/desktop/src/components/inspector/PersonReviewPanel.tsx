import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ChevronDown, ChevronRight, CircleUserRound, Settings2, X } from "lucide-react";
import { confirmFolderPerson, getFolderPersonReferenceAsset, getHistoricalLink, getHistoricalReferenceAsset, getPersonTagLink, getPersonTagOverride, linkHistoricalPerson, listCustomTags, listHistoricalPeople, listPersonInstances, listPersonReviews, resetFolderPerson, setPersonReview, setPersonTagLink, setPersonTagOverride, unlinkHistoricalPerson } from "@/lib/api";
import { Thumbnail } from "@/components/browsing/Thumbnail";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary, FolderPerson, PersonInstance, PersonReviewDecision } from "@/types";
import { usePeopleWorkspace } from "@/components/people/PeopleContext";

interface Props { folderPath: string; asset?: AssetSummary; person: FolderPerson }
const DECISIONS: Array<[PersonReviewDecision,string]> = [["belongs","属于"],["doesNotBelong","不属于"],["deferred","暂缓"],["pending","待确认"]];

export function PersonReviewPanel({ folderPath, asset, person }: Props) {
  const queryClient = useQueryClient();
  const workspace = usePeopleWorkspace();
  const rowsRef = useRef<HTMLDivElement>(null);
  useEffect(() => { rowsRef.current?.querySelector(".person-instance-row.is-selected")?.scrollIntoView({ block: "nearest" }); }, [workspace?.selectedInstanceId]);
  const [open, setOpen] = useState(true);
  const [manage, setManage] = useState(false);
  const [name, setName] = useState(person.displayName ?? "");
  const [referenceId, setReferenceId] = useState(person.referenceInstanceId ?? "");
  const [checked, setChecked] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [historyChoice, setHistoryChoice] = useState("new");
  const [historyChecked, setHistoryChecked] = useState(false);
  const [tagChoice, setTagChoice] = useState("");
  const [error, setError] = useState<string>();
  const instances = useQuery({ queryKey: ["person-instances",folderPath,asset?.path], queryFn: () => listPersonInstances(folderPath,asset!.path), enabled: Boolean(asset) });
  const reviews = useQuery({ queryKey: ["person-reviews",folderPath,person.id], queryFn: () => listPersonReviews(folderPath,person.id) });
  const historical = useQuery({ queryKey: ["historical-people"], queryFn: listHistoricalPeople, enabled: manage });
  const historicalLink = useQuery({ queryKey: ["historical-link",folderPath,person.id], queryFn: () => getHistoricalLink(folderPath,person.id), enabled: manage });
  const tagLink = useQuery({ queryKey: ["person-tag-link",folderPath,person.id], queryFn: () => getPersonTagLink(folderPath,person.id), enabled: manage && Boolean(historicalLink.data) });
  const tagOverride = useQuery({ queryKey: ["person-tag-override",folderPath,person.id,asset?.path], queryFn: () => getPersonTagOverride(folderPath,person.id,asset!.path), enabled: manage && Boolean(historicalLink.data) && Boolean(asset) });
  const tags = useQuery({ queryKey: ["custom-tags"], queryFn: listCustomTags, enabled: manage && Boolean(historicalLink.data) });
  useEffect(() => { setTagChoice(tagLink.data?.tagId?.toString() ?? ""); }, [tagLink.data?.tagId]);
  const currentReference = useQuery({ queryKey: ["folder-person-reference",folderPath,person.id,person.revision], queryFn: () => getFolderPersonReferenceAsset(folderPath,person.id), enabled: manage && person.identityConfirmed });
  const historyReference = useQuery({ queryKey: ["historical-reference",historyChoice], queryFn: () => getHistoricalReferenceAsset(historyChoice), enabled: manage && historyChoice !== "new" });
  const current = new Map(reviews.data?.filter(item => item.instance.assetPath === asset?.path).map(item => [item.instance.id,item]));
  const run = async (operation: () => Promise<unknown>, navigate = false) => {
    setBusy(true); setError(undefined);
    try {
      await operation();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["asset-tag-assignments"] }),
        queryClient.invalidateQueries({ queryKey: ["tag-sync-status"] }),
      ]);
      await workspace?.changed(asset,navigate);
      return true;
    }
    catch (cause) { setError(String(cause)); await workspace?.changed().catch(() => undefined); return false; }
    finally { setBusy(false); }
  };
  const beginDraw = (mode: "face" | "body", instance?: PersonInstance) => {
    if (!workspace) return;
    workspace.setEditInstance(instance); workspace.setDraw(mode);
    useWorkspaceStore.getState().setView("loupe");
  };
  const close = () => { if (!busy && (!dirty || window.confirm("放弃尚未保存的人物资料修改？"))) { setManage(false); setDirty(false); } };
  const references = (reviews.data ?? []).filter(item => item.decision === "belongs" && item.instance.faceBox && !item.instance.needsReview);
  const panelError = error ?? (instances.error ? String(instances.error) : reviews.error ? String(reviews.error) : undefined);
  return <section className="person-review-panel">
    <button className="person-review-title" onClick={() => setOpen(value => !value)} aria-expanded={open}>
      <span>{open ? <ChevronDown size={14} /> : <ChevronRight size={14} />} 照片审阅</span>
      <span className="person-review-title__count">{(instances.data ?? []).length}</span>
    </button>
    {open ? <div ref={rowsRef} className="person-review-content">
      <div className="person-review-summary">
        <span className="person-review-summary__icon"><CircleUserRound size={18} /></span>
        <span className="person-review-summary__text"><strong>{person.displayName || "匿名人物"}</strong><small>{person.identityConfirmed ? "身份已确认" : "身份待确认"} · {person.referenceInstanceId ? "已有参考" : "未选参考"}</small></span>
        <button className="person-review-summary__manage" title="管理身份与参考" aria-label="管理身份与参考" onClick={() => { setName(person.displayName ?? ""); setReferenceId(person.referenceInstanceId ?? ""); setChecked(false); setDirty(false); setManage(true); }}><Settings2 size={15} /></button>
      </div>
      {asset ? <>
        <div className="person-draw-actions"><button disabled={busy} onClick={() => beginDraw("face")}>＋ 人脸框</button><button disabled={busy} onClick={() => beginDraw("body")}>＋ 人体框</button></div>
        {workspace?.draw ? <p className="person-review-hint is-active">在照片上拖动以画出{workspace.draw === "face" ? "人脸" : "人体"}框{workspace.editInstance ? "；保存后需重新核对归属" : ""}。<button onClick={() => { workspace.setDraw(undefined); workspace.setEditInstance(undefined); }}>取消</button></p> : <p className="person-review-hint">选择照片中的人物框，逐一核对归属。</p>}
        {instances.isLoading || reviews.isLoading ? <p>正在读取人物记录…</p> : null}
        {(instances.data ?? []).map((instance,index) => {
          const review = current.get(instance.id);
          const stale = instance.needsReview || instance.sourceRevision !== `${asset.sizeBytes}:${asset.modifiedAtMs}`;
          return <div className={`person-instance-row ${workspace?.selectedInstanceId === instance.id ? "is-selected" : ""}`} key={instance.id}>
            <div className="person-instance-row__heading"><button onClick={() => workspace?.selectInstance(instance.id)}>人物 {index+1}</button><span>{instance.faceBox ? "人脸" : ""}{instance.faceBox && instance.bodyBox ? " · " : ""}{instance.bodyBox ? "人体" : ""}</span></div>
            <small className="person-instance-row__status">{review ? `当前：${DECISIONS.find(([value]) => value === review.decision)?.[1]}` : "尚未关联此人物"}</small>
            {stale ? <small className="person-instance-row__stale">源图已变化；请重画后复核。</small> : null}
            <div className="person-instance-row__edit"><button disabled={busy} onClick={() => beginDraw("face",instance)}>修正人脸</button><button disabled={busy} onClick={() => beginDraw("body",instance)}>关联／修正人体</button></div>
            <div className="person-instance-row__decisions">{DECISIONS.map(([value,label]) => <button key={value} className={review?.decision === value ? "is-current" : ""} disabled={busy || stale || reviews.isLoading || review?.decision === value}
              onClick={() => { workspace?.selectInstance(instance.id); void run(() => setPersonReview(folderPath,instance.id,person.id,value,review?.revision ?? 0),true); }}>{label}</button>)}</div>
          </div>;
        })}
      </> : <p>选择照片后添加、选择及审阅人物实例。</p>}
      {panelError ? <p role="alert" className="person-review-error">{panelError}</p> : null}
    </div> : null}
    {manage ? createPortal(<div className="person-management-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) close(); }} onKeyDown={event => { event.stopPropagation(); if(event.key === "Escape") close(); }}>
      <section className="person-management" role="dialog" aria-modal="true" aria-label="人物身份与参考管理">
        <header><div><span className="person-management__eyebrow">人物资料</span><h2>身份与参考</h2></div><button className="person-management__close" aria-label="关闭" disabled={busy} onClick={close}><X size={18} /></button></header>
        <p className="person-management__intro">管理当前场次的人物身份、参考照片与历史关联。逐张照片的归属仍需单独审阅。</p>
        <div className="person-management__section-heading"><span>01</span><h3>场次身份</h3></div>
        <label>姓名或场次称呼<input autoFocus aria-label="场次人物称呼" value={name} onChange={event => { setName(event.target.value); setDirty(true); }} /></label>
        <h3 className="person-management__subheading">选择已核对的人脸参考</h3>
        {!references.length ? <p>请先在照片中将一个清晰人脸实例审阅为“属于”。</p> : references.map(item => <label className="person-reference-choice" key={item.instance.id}>
          <input type="radio" name="reference" checked={referenceId === item.instance.id} onChange={() => { setReferenceId(item.instance.id); setChecked(false); setDirty(true); }} />
          {item.instance.assetPath.split(/[\\/]/).at(-1)} · {item.instance.id.slice(-8)}
        </label>)}
        <label><input type="checkbox" checked={checked} onChange={event => { setChecked(event.target.checked); setDirty(true); }} />已核对参考为此人且人脸清晰</label>
        <footer><button className="person-management__primary" disabled={busy || !name.trim() || !checked || !references.some(item => item.instance.id === referenceId)} onClick={() => void run(() => confirmFolderPerson(person,name,referenceId)).then(ok => { if(ok) { setDirty(false); setManage(false); } })}>保存并确认场次身份</button>
          <button disabled={busy} onClick={() => void run(() => resetFolderPerson(person,name)).then(ok => { if(ok) { setDirty(false); setManage(false); } })}>{person.identityConfirmed ? "撤销身份确认并清除参考" : "仅保存待确认称呼"}</button></footer>
        {person.identityConfirmed && person.referenceInstanceId ? <section className="person-history-link">
          <div className="person-management__section-heading"><span>02</span><h3>历史身份</h3></div>
          <p>当前场次：{person.displayName} · 参考 {references.find(item => item.instance.id === person.referenceInstanceId)?.instance.assetPath.split(/[\\/]/).at(-1) ?? "已保存"}</p>
          {historicalLink.isLoading || historical.isLoading ? <p>正在读取历史身份…</p> : null}
          {historicalLink.data ? <p>当前关联：{historical.data?.find(item => item.id === historicalLink.data)?.displayName ?? historicalLink.data} <button disabled={busy} onClick={() => void run(() => unlinkHistoricalPerson(person)).then(async ok => { if (ok) await historicalLink.refetch(); })}>撤销历史关联</button></p> : <p>尚未关联历史身份。</p>}
          <label><input type="radio" name="historical-person" checked={historyChoice === "new"} onChange={() => { setHistoryChoice("new"); setHistoryChecked(false); }} />建立新的历史人物</label>
          {(historical.data ?? []).map(item => <label key={item.id} className="person-reference-choice"><input type="radio" name="historical-person" checked={historyChoice === item.id} onChange={() => { setHistoryChoice(item.id); setHistoryChecked(false); }} />{item.displayName} · 历史参考 {item.referenceAssetPath.split(/[\\/]/).at(-1)}</label>)}
          <div className="person-history-previews">
            <div><strong>当前场次参考</strong>{currentReference.data ? <Thumbnail asset={currentReference.data} priority="visible" /> : <small>{currentReference.isLoading ? "正在读取参考…" : "参考不可用或源图已变化"}</small>}</div>
            {historyChoice !== "new" ? <div><strong>历史人物参考</strong>{historyReference.data ? <Thumbnail asset={historyReference.data} priority="visible" /> : <small>{historyReference.isLoading ? "正在读取历史参考…" : "历史参考不可用或源图已变化"}</small>}</div> : null}
          </div>
          <label><input type="checkbox" checked={historyChecked} onChange={event => setHistoryChecked(event.target.checked)} />{historyChoice === "new" ? "已核对当前参考并确认建立历史人物" : "已核对当前与所选历史参考，确认是同一人"}</label>
          <button disabled={busy || !historyChecked || !currentReference.data || (historyChoice !== "new" && !historyReference.data) || historical.isLoading || historicalLink.isLoading || historyChoice === historicalLink.data}
            onClick={() => void run(() => linkHistoricalPerson(person, historyChoice === "new" ? undefined : historyChoice)).then(async ok => { if (ok) { setHistoryChecked(false); await historical.refetch(); await historicalLink.refetch(); } })}>
            {historyChoice === "new" ? "建立并关联历史人物" : "确认关联此历史人物"}
          </button>
        </section> : null}
        {historicalLink.data ? <section className="person-tag-link">
          <div className="person-management__section-heading"><span>03</span><h3>同步人物标签</h3></div>
          <p>只为已确认“属于”的照片添加标签；其他手工和 sidecar 标签仍各自保留。</p>
          <p>当前场次已确认照片：{new Set((reviews.data ?? []).filter(item => item.decision === "belongs" && !item.instance.needsReview).map(item => item.instance.assetPath)).size} 张。相同历史人物的其他场次也会同步。</p>
          <label>选择已有标签<select aria-label="人物同步标签" value={tagChoice} onChange={event => setTagChoice(event.target.value)}>
            <option value="">不选择标签</option>{(tags.data ?? []).map(tag => <option key={tag.id} value={tag.id}>{tag.path}</option>)}
          </select></label>
          <p>当前：{tagLink.data?.enabled ? `同步到 ${tags.data?.find(tag => tag.id === tagLink.data?.tagId)?.path ?? "标签已删除"}` : "未启用"}</p>
          <button disabled={busy || !tagChoice || tagLink.isLoading || tags.isLoading} onClick={() => void run(() => setPersonTagLink(person, Number(tagChoice), true, tagLink.data?.revision ?? 0)).then(async ok => { if (ok) await tagLink.refetch(); })}>启用／更改同步</button>
          <button disabled={busy || !tagLink.data?.enabled} onClick={() => void run(() => setPersonTagLink(person, tagLink.data?.tagId ?? null, false, tagLink.data?.revision ?? 0)).then(async ok => { if (ok) await tagLink.refetch(); })}>关闭同步</button>
          {asset && tagLink.data?.enabled && (reviews.data ?? []).some(item => item.instance.assetPath === asset.path && item.decision === "belongs") ? <p>
            当前照片：{tagOverride.data?.suppressed ? "已停止此人物同步" : "按已确认归属同步"}。
            <button disabled={busy || tagOverride.isLoading} onClick={() => void run(() => setPersonTagOverride(person, asset.path, !tagOverride.data?.suppressed, tagOverride.data?.revision ?? 0)).then(async ok => { if (ok) await tagOverride.refetch(); })}>
              {tagOverride.data?.suppressed ? "恢复此图同步" : "停止此人物在此图同步"}
            </button>
          </p> : null}
        </section> : null}
        {panelError ? <p role="alert">{panelError}</p> : null}
      </section>
    </div>,document.body) : null}
  </section>;
}
