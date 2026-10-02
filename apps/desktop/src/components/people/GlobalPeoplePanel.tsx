import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getPersonOperation, getPersonTupleAsset, reviewPersonTuples, saveGlobalPerson, setGlobalPersonReference, startPeopleGrouping, getGlobalPersonReferences, listCustomTags, setGlobalPersonTag } from "@/lib/api";
import type { AssetSummary, GlobalPerson, PersonReviewDecision, PersonTuple } from "@/types";
import { useGlobalPeopleContext, uniquePersonTuples } from "./GlobalPeopleContext";
import { PersonAnalysisControls } from "./PersonAnalysisControls";
import { Thumbnail } from "../browsing/Thumbnail";
import { useWorkspaceStore } from "@/store";
import styles from "./GlobalPeoplePanel.module.css";
import { Check, ChevronRight, FolderOpen, Users } from "lucide-react";

const labels: Record<PersonReviewDecision, string> = { pending: "待确认", belongs: "已确认", doesNotBelong: "不属于", deferred: "暂缓" };
function TuplePreview({ tuple }: { tuple: PersonTuple }) {
  const [ratio, setRatio] = useState(1.5);
  const loaded = useCallback((size: { width: number; height: number }) => setRatio(size.width / size.height), []);
  const asset = useQuery({ queryKey: ["tuple-asset", tuple.assetPath, tuple.sourceRevision], queryFn: () => getPersonTupleAsset(tuple.assetPath), staleTime: Infinity, retry: false });
  const box = tuple.faceBox ?? tuple.bodyBox;
  return <span className={styles.preview} style={{ aspectRatio: ratio }}>{asset.data ? <Thumbnail asset={asset.data} crossFolder priority="visible" onImageLoad={loaded} /> : <span>实例</span>}{box ? <i style={{ left: `${box[0] * 100}%`, top: `${box[1] * 100}%`, width: `${box[2] * 100}%`, height: `${box[3] * 100}%` }} /> : null}</span>;
}
function GroupCover({ asset }: { asset: AssetSummary }) {
  const ref = useRef<HTMLSpanElement>(null); const [visible, setVisible] = useState(false);
  useEffect(() => { const observer = new IntersectionObserver(entries => setVisible(entries.some(e => e.isIntersecting))); if (ref.current) observer.observe(ref.current); return () => observer.disconnect(); }, []);
  return <span ref={ref} className={styles.cover}><Thumbnail asset={asset} enabled={visible} priority="visible" /></span>;
}
export function GlobalPeoplePanel({ sessionId, activeAsset, selectedAssetPaths = [] }: { sessionId?: string; activeAsset?: AssetSummary; selectedAssetPaths?: string[] }) {
  const state = useGlobalPeopleContext()!;
  const { workspace, people, group, members, selected, folderPath, refresh } = state;
  const [target, setTarget] = useState(""); const [name, setName] = useState(""); const [editing, setEditing] = useState<GlobalPerson>();
  const [quickTarget, setQuickTarget] = useState("");
  const panelRef = useRef<HTMLDivElement>(null);
  useEffect(() => { setQuickTarget(""); if (panelRef.current) panelRef.current.scrollTop = 0; }, [activeAsset?.path, group?.id]);
  const editingCurrent = people.find(p => p.id === editing?.id);
  const [tagChoice, setTagChoice] = useState("");
  const references = useQuery({ queryKey: ["global-reference", editing?.id], queryFn: () => getGlobalPersonReferences(editing!.id), enabled: !!editing });
  const tags = useQuery({ queryKey: ["custom-tags"], queryFn: listCustomTags, enabled: !!editing });
  useEffect(() => setTagChoice(editingCurrent?.tagId?.toString() ?? ""), [editingCurrent?.id, editingCurrent?.tagId]);
  const [findOpen, setFindOpen] = useState(false); const [findIds, setFindIds] = useState<Set<string>>(new Set());
  const [threshold, setThreshold] = useState(.4); const [replace, setReplace] = useState(false); const [refreshFeatures, setRefreshFeatures] = useState(false);
  const [busy, setBusy] = useState(false); const [error, setError] = useState(""); const [message, setMessage] = useState(""); const [page, setPage] = useState(0);
  const scope = useRef(folderPath); scope.current = folderPath;
  const operation = useQuery({ queryKey: ["person-operation"], queryFn: getPersonOperation, refetchInterval: 1000 });
  const active = !!operation.data && ["preparing", "analysing", "clustering", "downloading"].includes(operation.data.state);
  useEffect(() => { setTarget(group?.personId ?? ""); setPage(0); setReplace(false); setMessage(""); setError(""); }, [folderPath, group?.id, group?.personId, state.filter?.decision]);
  const run = async (action: () => Promise<void>) => { setBusy(true); setError(""); try { await action(); } catch (e) { setError(String(e)); } finally { setBusy(false); } };
  const apply = async (tuples: PersonTuple[], decision: PersonReviewDecision, personId = target) => {
    if (!workspace || !personId || busy) return;
    const origin = folderPath;
    state.setHeldAsset(activeAsset);
    await run(async () => {
      const count = await reviewPersonTuples(workspace, tuples, personId, decision, replace); await refresh();
      if (scope.current !== origin) return;
      state.setSelected(new Set()); setMessage(`${labels[decision]} · ${count} 个实例。照片留在原处，可继续核对。`);
      if (decision !== "doesNotBelong" && (decision === "pending" || group?.personId !== personId)) state.setFilter({ groupId: `person:${personId}`, decision: decision === "pending" ? "pending" : null },activeAsset);
    });
  };
  const start = async (ids: string[] | null) => { if (!sessionId || !folderPath) return; await run(async () => { await startPeopleGrouping(sessionId, folderPath, ids, threshold, refreshFeatures); await operation.refetch(); }); };
  const chosen = members.filter(t => selected.has(t.id));
  const activeTuples = uniquePersonTuples(workspace?.groups.flatMap(g => g.members).filter(t => t.assetPath === activeAsset?.path) ?? []);
  const selectedPaths = new Set(selectedAssetPaths);
  const batchReview = selectedPaths.size > 1;
  // A Grid selection is photo-level; keep its review scope inside the current
  // group and decision filter so other people in the same photos are untouched.
  const batchTuples = batchReview ? uniquePersonTuples((group ? members : workspace?.groups.flatMap(g => g.members) ?? []).filter(t => selectedPaths.has(t.assetPath))) : [];
  const batches = new Map<string, PersonTuple[]>();
  for (const tuple of batchTuples) {
    const key = tuple.personId ?? "unknown";
    const items = batches.get(key) ?? [];
    items.push(tuple);
    batches.set(key, items);
  }
  const person = people.find(p => p.id === target);
  const openTuple = async (t: PersonTuple, add = false, range = false) => { state.choose(t.id, add, range); await run(async () => { const asset = await getPersonTupleAsset(t.assetPath); if (scope.current === folderPath) { useWorkspaceStore.getState().select(asset.id); useWorkspaceStore.getState().setView("loupe"); } }); };
  return <div ref={panelRef} className={styles.panel} aria-label="全局人物工作区">
    <nav className={styles.navigation} aria-label="人物工作区导航">
      <button aria-pressed={state.surface === "library"} onClick={state.openLibrary}><Users size={17} /><span>全部人物<small>跨文件夹的确认记录</small></span><b>{people.length}</b></button>
      <button aria-pressed={state.surface === "folder"} disabled={!folderPath} onClick={() => state.setSurface("folder")}><FolderOpen size={17} /><span>当前文件夹<small>{folderPath?.split(/[\\/]/).at(-1) ?? "先打开一个文件夹"}</small></span><ChevronRight size={14} /></button>
    </nav>
    {state.surface === "folder" ? <section className={styles.quickReview} aria-label="看图确认">
      <div className={styles.reviewHeading}><strong>看图确认</strong><span>{batchReview ? `已选 ${selectedPaths.size} 张 · ${batchTuples.length} 个实例` : `${activeTuples.length} 人`}</span></div>
      <p className={styles.hint}>{batchReview ? "按人物批量处理选中照片中的实例，数字随 Grid 选择同步。" : activeTuples.length ? "点选图中人物，再确认归属。完成后照片保留在原处。" : group ? "在主窗口打开一张照片，开始确认。" : "先选下方人物分组，再在主窗口查看照片。"}</p>
      {batchReview ? [...batches].map(([key, tuples]) => {
        const identity = people.find(p => p.id === tuples[0].personId);
        const chosenId = quickTarget || identity?.id || target;
        const needsReview = tuples.some(t => t.needsReview);
        return <div key={key} className={styles.quickTuple}>
          <div className={styles.quickSelect}><span className={styles.instanceNumber} aria-label={`${identity?.displayName ?? "未知人物"}已选实例数`}>{tuples.length}</span><span><strong>{identity?.displayName ?? "未知人物"}</strong><small>{new Set(tuples.map(t => t.assetPath)).size} 张照片 · {tuples.length} 个实例</small></span></div>
          <div className={styles.quickActions}><button className={styles.confirm} disabled={busy || !chosenId || needsReview} onClick={() => void apply(tuples, "belongs", chosenId)}>批量确认{people.find(p => p.id === chosenId)?.displayName ? `为 ${people.find(p => p.id === chosenId)?.displayName}` : "人物"}</button><button disabled={busy || !chosenId} onClick={() => void apply(tuples, "doesNotBelong", chosenId)}>批量不属于</button><button disabled={busy || !chosenId} onClick={() => void apply(tuples, "deferred", chosenId)}>批量暂缓</button></div>
          {needsReview ? <small>选中实例包含需要复核的人物框，请先在“标记与修正”中复核。</small> : null}
        </div>;
      }) : activeTuples.map((t, i) => {
        const relation = group?.members.find(m => m.id === t.id) ?? t;
        const identity = people.find(p => p.id === relation.personId);
        const chosenId = quickTarget || identity?.id || target;
        return <div key={t.id} className={`${styles.quickTuple} ${selected.has(t.id) ? styles.selected : ""}`}>
          <button className={styles.quickSelect} aria-pressed={selected.has(t.id)} onClick={() => { state.choose(t.id); state.setShowBoxes(true); useWorkspaceStore.getState().setView("loupe"); }}><span className={styles.instanceNumber}>{i + 1}</span><span><strong>{identity?.displayName ?? "未知人物"}</strong><small>{relation.decision ? labels[relation.decision] : "请选择一个名字"}</small></span>{relation.decision === "belongs" ? <Check size={16} /> : null}</button>
          <div className={styles.quickActions}><button className={styles.confirm} disabled={busy || !chosenId || relation.needsReview} onClick={() => void apply([relation], "belongs", chosenId)}>确认{people.find(p => p.id === chosenId)?.displayName ? `为 ${people.find(p => p.id === chosenId)?.displayName}` : "人物"}</button><button disabled={busy || !chosenId} onClick={() => void apply([relation], "doesNotBelong", chosenId)}>不属于</button><button disabled={busy || !chosenId} onClick={() => void apply([relation], "deferred", chosenId)}>暂缓</button></div>
          {relation.needsReview ? <small>请先在“标记与修正”中复核人物框。</small> : null}
        </div>;
      })}
      {batchReview && !batchTuples.length ? <p>选中照片中没有符合当前分组与过滤条件的实例。</p> : null}
      {(batchReview ? batchTuples.length : activeTuples.length) ? <label>改为其他人物<select aria-label="看图指定人物" value={quickTarget} onChange={e => setQuickTarget(e.target.value)}><option value="">使用各实例的候选人物</option>{people.map(p => <option key={p.id} value={p.id}>{p.displayName}</option>)}</select></label> : null}
      {message ? <p className={styles.feedback} role="status"><Check size={14} />{message}</p> : null}
    </section> : null}
    {state.surface === "folder" ? <details className={styles.setup} open={!workspace?.groups.length}><summary>整理当前文件夹</summary>
      <strong title={folderPath}>当前文件夹 · {folderPath?.split(/[\\/]/).at(-1) ?? "未选择"}</strong>
      <div className={styles.counts}><span>未知身份 <b>{workspace?.unknownCount ?? 0}</b> 组</span><span>已知身份 <b>{workspace?.knownCount ?? 0}</b> 人</span></div>
      <div className={styles.actions}><button disabled={!folderPath || busy || active} onClick={() => void start(null)}>自动分组</button><button disabled={!folderPath || busy || active} onClick={() => setFindOpen(v => !v)}>寻找人物</button></div>
      {findOpen ? <div className={styles.find}><p>只寻找勾选人物，结果进入同一套待确认审阅。</p>{people.map(p => <label key={p.id}><input type="checkbox" checked={findIds.has(p.id)} onChange={e => setFindIds(old => { const next = new Set(old); if (e.target.checked) next.add(p.id); else next.delete(p.id); return next; })} />{p.displayName} · {p.referenceInstanceIds.length} 个参考</label>)}{!people.length ? <p>先在下方全局人物库创建人物并确认参考。</p> : null}<button disabled={!findIds.size || busy || active} onClick={() => void start([...findIds])}>开始寻找</button></div> : null}
      <details><summary>识别选项</summary><label>候选相似度 <input aria-label="候选相似度" type="number" min={0} max={1} step={.025} value={threshold} onChange={e => setThreshold(Number(e.target.value))} /></label><small>越低返回候选越多；相似度不代表身份已确认。</small><label><input type="checkbox" checked={refreshFeatures} onChange={e => setRefreshFeatures(e.target.checked)} />重新提取当前文件夹特征</label></details>
      <PersonAnalysisControls sessionId={sessionId} folderPath={folderPath} modelsOnly />
    </details> : null}
    <details className={styles.registry} open={state.surface === "library"}><summary>人物资料 · 新建与管理</summary><p>姓名与参考在所有文件夹共用。</p>
      <select aria-label="编辑全局人物" value={editing?.id ?? ""} onChange={e => { const p = people.find(p => p.id === e.target.value); setEditing(p); setName(p?.displayName ?? ""); }}><option value="">新建人物</option>{people.map(p => <option key={p.id} value={p.id}>{p.displayName}</option>)}</select>
      <form onSubmit={e => { e.preventDefault(); void run(async () => { const p = await saveGlobalPerson(name, editingCurrent); await refresh(); setTarget(p.id); setEditing(undefined); setName(""); setMessage("人物资料已全 App 同步。"); }); }}><input aria-label="全局人物姓名" placeholder="人物姓名" value={name} onChange={e => setName(e.target.value)} /><button disabled={busy || !name.trim()}>{editing ? "保存全局姓名" : "创建人物"}</button></form>
      {editingCurrent ? <><label>人物标签<select aria-label="全局人物标签" value={tagChoice} onChange={e => setTagChoice(e.target.value)}><option value="">关闭标签同步</option>{tags.data?.map(t => <option key={t.id} value={t.id}>{t.path}</option>)}</select></label><button disabled={busy} onClick={() => void run(async () => { await setGlobalPersonTag(editingCurrent, tagChoice ? Number(tagChoice) : null); await refresh(); setMessage("人物标签设置已同步到各文件夹的已确认照片。"); })}>保存标签同步</button><p>全局参考 · {editingCurrent.referenceInstanceIds.length} 个</p>{references.data?.map(t => <div key={t.id} className={styles.tuple} title={t.assetPath}><TuplePreview tuple={t} /><span>{t.assetPath.split(/[\\/]/).at(-1)}</span><button disabled={busy} onClick={() => void run(async () => { await setGlobalPersonReference(editingCurrent, t.id, false); await refresh(); })}>移除参考</button></div>)}</> : null}
    </details>
    {state.surface === "folder" ? <><section>
      <div className={styles.actions}><strong>人物分组</strong>{state.filter ? <button onClick={() => state.setFilter(undefined)}>显示全部照片</button> : null}</div>
      {!workspace?.groups.length ? <p>点击“自动分组”，或在照片中手工补充人物实例。</p> : null}
      <details open={!group}><summary>{group ? "切换分组" : "已知人物与匿名组"}</summary><div className={styles.groups}>{workspace?.groups.map((g, i) => <button key={g.id} className={state.filter?.groupId === g.id ? styles.selected : ""} aria-pressed={state.filter?.groupId === g.id} onClick={() => state.setFilter({ groupId: g.id })}>
        {g.cover ? <GroupCover asset={g.cover} /> : null}<span><strong>{people.find(p => p.id === g.personId)?.displayName ?? `匿名组 ${workspace.groups.slice(0, i + 1).filter(item => !item.personId).length}`}</strong><small>{new Set(g.members.map(t => t.assetPath)).size} 张 · {g.members.length} 个实例{g.personId ? ` · ${g.members.filter(t => t.decision === "pending").length} 待确认` : ""}</small></span></button>)}</div></details>
      {workspace ? <small>未检测到人脸 {workspace.noFaceCount} 张 · 缺失或失效证据 {workspace.unavailableCount} 项</small> : null}
    </section>
    {group ? <section aria-label="实例归属审阅">
      <strong>{group.personId ? people.find(p => p.id === group.personId)?.displayName : "匿名组"} · 实例归属</strong>
      {group.personId ? <label>显示 <select aria-label="实例审阅过滤" value={state.filter?.decision ?? "all"} onChange={e => state.setFilter({ groupId: group.id, decision: e.target.value === "all" ? null : e.target.value as PersonReviewDecision })}><option value="all">全部实例</option>{Object.entries(labels).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</select></label> : null}
      <label>指定人物<select aria-label="实例指定人物" value={target} onChange={e => setTarget(e.target.value)}><option value="">选择全局人物</option>{people.map(p => <option key={p.id} value={p.id}>{p.displayName}</option>)}</select></label>
      <details className={styles.batch}><summary>批量操作与替换归属</summary><div className={styles.actions}><button disabled={!target || busy} onClick={() => void apply(group.members, "pending")}>整组转为待确认</button><button onClick={() => state.setSelected(new Set(members.map(t => t.id)))}>全选实例</button><button onClick={() => state.setSelected(new Set())}>清除选择</button></div>
      <small>已选 {chosen.length} 个实例。Ctrl 多选，Shift 连选；每行只对应框出的一个人。</small>
      <div className={styles.actions}>{(["pending", "belongs", "doesNotBelong", "deferred"] as const).map(d => <button key={d} disabled={!target || !chosen.length || busy} onClick={() => void apply(chosen, d)}>{d === "pending" ? "指定为待确认" : labels[d]}</button>)}</div>
      <label><input type="checkbox" checked={replace} onChange={e => setReplace(e.target.checked)} />替换选中实例已有的其他已确认人物</label>
      </details>
      {members.slice(page * 30, (page + 1) * 30).map((t, i) => <div key={t.id} className={`${styles.tuple} ${selected.has(t.id) ? styles.selected : ""}`}>
        <input type="checkbox" aria-label={`选择实例 ${page * 30 + i + 1}`} checked={selected.has(t.id)} onChange={() => state.choose(t.id, true)} />
        <button className={styles.tupleMain} onClick={e => void openTuple(t, e.ctrlKey || e.metaKey, e.shiftKey)}><TuplePreview tuple={t} /><span>{t.assetPath.split(/[\\/]/).at(-1)}<small>{t.faceBox ? "面部" : ""}{t.faceBox && t.bodyBox ? " + " : ""}{t.bodyBox ? "人体" : ""} · {t.decision ? labels[t.decision] : "未知身份"}{t.score != null ? ` · ${t.score.toFixed(3)}` : ""}</small>{t.needsReview ? <small>需要重画复核</small> : null}</span></button>
        {person && t.personId === person.id && t.decision === "belongs" ? <button className={styles.reference} disabled={busy} onClick={() => void run(async () => { await setGlobalPersonReference(person, t.id, !person.referenceInstanceIds.includes(t.id)); await refresh(); })}>{person.referenceInstanceIds.includes(t.id) ? "移除参考" : "设为参考"}</button> : null}
      </div>)}
      {members.length > 30 ? <div className={styles.actions}><button disabled={page === 0} onClick={() => setPage(p => p - 1)}>上一页</button><span>{page + 1} / {Math.ceil(members.length / 30)}</span><button disabled={(page + 1) * 30 >= members.length} onClick={() => setPage(p => p + 1)}>下一页</button></div> : null}
    </section> : null}
    <details className={styles.setup} aria-label="当前照片人物实例"><summary>标记与修正 · {activeTuples.length} 个实例</summary><label><input type="checkbox" checked={state.showBoxes} onChange={e => state.setShowBoxes(e.target.checked)} />显示人物框</label>
      <div className={styles.actions}>{(["face", "body"] as const).map(kind => <button key={kind} disabled={!activeAsset || busy} onClick={() => { state.setEdit(undefined); state.setDraw(kind); state.setShowBoxes(true); useWorkspaceStore.getState().setView("loupe"); }}>＋ {kind === "face" ? "人脸" : "人体"}实例</button>)}</div>
      {activeTuples.map((t, i) => <div key={t.id} className={styles.actions}><button onClick={() => { const g = workspace?.groups.find(g => g.members.some(m => m.id === t.id)); if (g && state.filter?.groupId !== g.id) state.setFilter({ groupId: g.id }); state.choose(t.id); }}>实例 {i + 1} · {people.find(p => p.id === t.personId)?.displayName ?? "未知"}</button>{(["face", "body"] as const).map(kind => <button key={kind} onClick={() => { state.setEdit(t); state.setDraw(kind); state.setShowBoxes(true); useWorkspaceStore.getState().setView("loupe"); }}>{kind === "face" ? "修正面部" : "关联／修正人体"}</button>)}</div>)}
      {state.draw ? <p>在大图拖动绘制{state.draw === "face" ? "面部" : "人体"}框。<button onClick={() => { state.setDraw(undefined); state.setEdit(undefined); }}>取消画框</button></p> : null}
    </details></> : null}
    {workspace?.notice ? <p className={styles.notice}>{workspace.notice}</p> : null}
    {message && state.surface === "library" ? <p role="status">{message}</p> : null}{error || state.catalog.error || state.result.error ? <p role="alert">{error || String(state.catalog.error ?? state.result.error)}</p> : null}
  </div>;
}
