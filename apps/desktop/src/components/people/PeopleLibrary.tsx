import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, Folder, FolderOpen, Search, Users } from "lucide-react";
import { getGlobalPersonFolders, getGlobalPersonGallery, getPersonTupleAsset } from "@/lib/api";
import type { GlobalPerson, PersonTuple } from "@/types";
import { Thumbnail } from "../browsing/Thumbnail";
import { useGlobalPeopleContext } from "./GlobalPeopleContext";
import styles from "./PeopleLibrary.module.css";

const fileName = (path: string) => path.split(/[\\/]/).at(-1);

export function PersonPhoto({ tuple, large = false, portrait = false }: { tuple: PersonTuple; large?: boolean; portrait?: boolean }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const [ratio, setRatio] = useState(1.5);
  const [previewError, setPreviewError] = useState<string>();
  useEffect(() => {
    const observer = new IntersectionObserver(entries => { if (entries.some(e => e.isIntersecting)) setVisible(true); });
    if (ref.current) observer.observe(ref.current);
    return () => observer.disconnect();
  }, []);
  const asset = useQuery({ queryKey: ["tuple-asset", tuple.assetPath, tuple.sourceRevision], queryFn: () => getPersonTupleAsset(tuple.assetPath), enabled: visible, staleTime: Infinity, retry: false });
  const box = tuple.faceBox ?? tuple.bodyBox;
  const cropScale = box ? Math.min(5, Math.max(1, (tuple.faceBox ? .36 : .8) / box[2])) : 1;
  const crop = portrait && box && !previewError ? { transform: `scale(${cropScale}) translate(${(0.5 - box[0] - box[2] / 2) * 100}%, ${(0.5 - box[1] - box[3] / 2) * 100}%)` } : {};
  const stale = tuple.needsReview || !!asset.data && tuple.sourceRevision !== `${asset.data.sizeBytes}:${asset.data.modifiedAtMs}`;
  return <span ref={ref} className={`${styles.photo} ${large ? styles.large : ""}`} style={{ aspectRatio: ratio, ...crop, ...(large ? { width: `min(100%, calc(max(240px, 100vh - 330px) * ${ratio}))` } : {}) }}>
    {asset.data ? <Thumbnail asset={asset.data} enabled={visible} large={large} crossFolder onPreviewError={setPreviewError} onImageLoad={size => setRatio(size.width / size.height)} /> : <span className={styles.photoPlaceholder}>{asset.isError ? "原图暂不可用" : "载入照片…"}</span>}
    {previewError ? <span className={styles.previewError}>{previewError.includes("budget") ? "原图过大，暂无法生成预览" : "预览暂不可用"}</span> : null}
    {box && asset.data && !stale && !previewError && !portrait ? <i className={styles.box} style={{ left: `${box[0] * 100}%`, top: `${box[1] * 100}%`, width: `${box[2] * 100}%`, height: `${box[3] * 100}%` }} /> : null}
    {stale ? <span className={styles.stale}>原图或标记已变化，需复核</span> : null}
  </span>;
}

function PersonCard({ person, onOpen }: { person: GlobalPerson; onOpen: () => void }) {
  const ref = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const observer = new IntersectionObserver(entries => { if (entries.some(e => e.isIntersecting)) setVisible(true); });
    if (ref.current) observer.observe(ref.current);
    return () => observer.disconnect();
  }, []);
  const gallery = useQuery({ queryKey: ["global-gallery", person.id, 0, 1], queryFn: () => getGlobalPersonGallery(person.id, 0, 1), enabled: visible });
  return <button ref={ref} className={styles.personCard} onClick={onOpen} aria-label={`浏览 ${person.displayName}`}>
    <span className={styles.portrait}>{gallery.data?.tuples[0] ? <PersonPhoto tuple={gallery.data.tuples[0]} portrait /> : <span className={styles.initial}>{person.displayName.slice(0, 1)}<Users size={22} strokeWidth={1} /></span>}</span>
    <span className={styles.personInfo}><strong>{person.displayName}</strong><span>{gallery.isError ? "记录载入失败，点击重试" : gallery.data ? `${gallery.data.total} 个确认标记 · ${gallery.data.folderCount} 个文件夹` : "读取确认记录…"}</span><small>{person.referenceInstanceIds.length ? `${person.referenceInstanceIds.length} 个识别参考` : "尚未设置识别参考"}</small></span>
  </button>;
}

function FolderPreview({ path }: { path: string }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const [previewError, setPreviewError] = useState<string>();
  useEffect(() => {
    const observer = new IntersectionObserver(entries => { if (entries.some(e => e.isIntersecting)) setVisible(true); });
    if (ref.current) observer.observe(ref.current);
    return () => observer.disconnect();
  }, []);
  const asset = useQuery({ queryKey: ["person-folder-cover", path], queryFn: () => getPersonTupleAsset(path), enabled: visible, staleTime: 60_000, retry: false });
  return <span ref={ref} className={styles.folderPreview}>
    {asset.data ? <Thumbnail asset={asset.data} enabled={visible} crossFolder onPreviewError={setPreviewError} /> : <span className={styles.photoPlaceholder}>{asset.isError ? "原图暂不可用" : "载入照片…"}</span>}
    {previewError ? <span className={styles.previewError}>预览暂不可用</span> : null}
    <span className={styles.folderBadge} aria-hidden="true"><Folder size={20} strokeWidth={1.5} /></span>
  </span>;
}

function PersonFolders({ person, onBack, onOpenFolder }: { person: GlobalPerson; onBack: () => void; onOpenFolder: (path: string) => Promise<void> }) {
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<string>();
  const [opening, setOpening] = useState<string>();
  const openingRef = useRef(false);
  const [error, setError] = useState<string>();
  const folders = useQuery({ queryKey: ["global-person-folders", person.id, page, 24], queryFn: () => getGlobalPersonFolders(person.id, page * 24, 24) });
  const open = async (path: string) => {
    if (openingRef.current) return;
    openingRef.current = true;
    setOpening(path); setError(undefined);
    try { await onOpenFolder(path); }
    catch (cause) { setError(`无法打开文件夹：${String(cause)}`); }
    finally { openingRef.current = false; setOpening(undefined); }
  };
  return <div className={styles.library} aria-label="人物文件夹">
    <header className={styles.header}><button className={styles.back} onClick={onBack}><ArrowLeft size={16} />全部人物</button><h1>{person.displayName}</h1><p>{folders.data ? `${folders.data.total} 个文件夹含有已确认照片` : "正在读取已确认记录…"} · 双击文件夹打开，左侧目录同步定位。</p></header>
    {error ? <p role="alert">{error}</p> : null}
    {folders.isError ? <div role="alert" className={styles.empty}><p>{String(folders.error)}</p><button onClick={() => void folders.refetch()}>重新载入</button></div> : null}
    {folders.isPending ? <p role="status">正在载入文件夹…</p> : null}
    {folders.data?.total === 0 ? <div className={styles.empty}><Users size={40} strokeWidth={1} /><h2>第一张确认，从看图开始</h2><p>在当前文件夹中选中具体的人并确认，来源文件夹就会汇集到这里。</p></div> : null}
    <div className={styles.gallery}>{folders.data?.folders.map(folder => <button
      key={folder.folderPath} className={`${styles.folderCard} ${selected === folder.folderPath ? styles.selectedFolder : ""}`}
      aria-label={`打开文件夹 ${folder.folderPath}`} aria-pressed={selected === folder.folderPath} disabled={!!opening}
      onClick={() => setSelected(folder.folderPath)} onDoubleClick={() => void open(folder.folderPath)}
      onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); void open(folder.folderPath); } }}>
      <FolderPreview key={folder.coverAssetPath} path={folder.coverAssetPath} />
      <span className={styles.folderInfo}><strong>{fileName(folder.folderPath)}</strong>
        <span>{folder.photoCount} 张已确认照片 · {folder.instanceCount} 个标记</span>
        <small title={folder.folderPath}>{folder.folderPath}</small>
        {opening === folder.folderPath ? <span role="status">正在打开…</span> : null}
      </span>
    </button>)}</div>
    {(folders.data?.total ?? 0) > 24 ? <nav className={styles.pagination} aria-label="人物文件夹分页"><button disabled={page === 0 || !!opening} onClick={() => setPage(p => p - 1)}>上一页</button><span>{page + 1} / {Math.ceil((folders.data?.total ?? 0) / 24)}</span><button disabled={!!opening || (page + 1) * 24 >= (folders.data?.total ?? 0)} onClick={() => setPage(p => p + 1)}>下一页</button></nav> : null}
  </div>;
}

export function PeopleLibrary({ onOpenFolder }: { onOpenFolder: (path: string) => Promise<void> }) {
  const state = useGlobalPeopleContext()!;
  const [search, setSearch] = useState("");
  const [personId, setPersonId] = useState<string>();
  const person = state.people.find(p => p.id === personId);
  const filtered = state.people.filter(p => p.displayName.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()));
  if (person) return <PersonFolders key={person.id} person={person} onBack={() => setPersonId(undefined)} onOpenFolder={onOpenFolder} />;
  return <div className={styles.library} aria-label="全局人物总览">
    <header className={styles.header}><div className={styles.headingRow}><h1>那些被记住的人</h1><span className={styles.libraryCount}>{state.people.length} 位人物</span></div><p>每一次确认，都在这里相聚。浏览所有文件夹里你标记过的人。</p></header>
    <div className={styles.libraryTools}><label className={styles.search}><Search size={17} /><input aria-label="搜索全局人物" placeholder="按姓名寻找人物" value={search} onChange={e => setSearch(e.target.value)} />{search ? <button aria-label="清除人物搜索" onClick={() => setSearch("")}>×</button> : null}</label><button className={styles.folderLink} disabled={!state.folderPath} onClick={() => state.setSurface("folder")}><FolderOpen size={16} />审阅当前文件夹</button></div>
    {state.catalog.isError ? <div role="alert" className={styles.empty}><p>人物库载入失败：{String(state.catalog.error)}</p><button onClick={() => void state.catalog.refetch()}>重新载入</button></div> : state.catalog.isPending ? <p role="status">正在载入人物库…</p> : !state.people.length ? <div className={styles.empty}><Users size={46} strokeWidth={1} /><h2>为照片里的人，留一个名字</h2><p>在左侧创建人物，再到文件夹中确认照片。以后，无论照片在哪个文件夹，都能从这里找到。</p><button disabled={!state.folderPath} onClick={() => state.setSurface("folder")}>开始整理当前文件夹</button></div> : !filtered.length ? <div className={styles.empty}><h2>没有找到“{search}”</h2><button onClick={() => setSearch("")}>查看全部人物</button></div> : <div className={styles.peopleGrid}>{filtered.map(p => <PersonCard key={p.id} person={p} onOpen={() => setPersonId(p.id)} />)}</div>}
  </div>;
}
