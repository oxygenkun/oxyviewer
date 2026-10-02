import { useEffect, useRef, useState } from "react";
import { Thumbnail } from "@/components/browsing/Thumbnail";
import { useQueryClient } from "@tanstack/react-query";
import { adoptPersonCluster } from "@/lib/api";
import type { AssetSummary, FolderPerson, PersonClusterFilter, PersonClusterSnapshot } from "@/types";
import styles from "./PersonClusters.module.css";

function Cover({ asset }: { asset: AssetSummary }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const observer = new IntersectionObserver(entries => setVisible(entries.some(e => e.isIntersecting)));
    if (ref.current) observer.observe(ref.current);
    return () => observer.disconnect();
  }, []);
  return <span ref={ref} className={styles.cover}><Thumbnail asset={asset} enabled={visible} priority="visible" /></span>;
}

export function PersonClusters({ folderPath, snapshot, people, selected, onSelect, onAdopted, error }: {
  folderPath: string;
  snapshot?: PersonClusterSnapshot | null;
  people: FolderPerson[];
  selected?: PersonClusterFilter;
  onSelect: (filter?: PersonClusterFilter) => void;
  onAdopted: (folder: string, person: FolderPerson) => void;
  error?: string;
}) {
  const client = useQueryClient();
  const [subject, setSubject] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [failure, setFailure] = useState("");
  const cluster = snapshot?.snapshotId === selected?.snapshotId ? snapshot?.clusters.find(c => c.id === selected?.clusterId) : undefined;
  useEffect(() => { setSubject(cluster?.people[0]?.id ?? ""); }, [cluster?.id, cluster?.people[0]?.id]);
  const adopt = async () => {
    if (!snapshot || !cluster) return;
    setBusy(true); setFailure(""); setMessage("");
    try {
      const result = await adoptPersonCluster(folderPath, snapshot.snapshotId, cluster.id, subject || undefined, name.trim() || undefined);
      await Promise.all(["person-clusters", "folder-people", "person-instances", "person-reviews", "assets"].map(key => client.invalidateQueries({ queryKey: [key] })));
      setMessage(`加入待确认 ${result.added} 个；保留原决定 ${result.preserved} 个；需单独核对 ${result.conflicted} 个。`);
      onAdopted(folderPath, result.person);
    } catch (cause) { setFailure(String(cause)); }
    finally { setBusy(false); }
  };
  return <section className={styles.panel} aria-label="匿名聚类">
    <div className="person-sidebar-heading"><strong>人物分组{snapshot ? ` · ${snapshot.clusters.length}` : ""}</strong>{selected ? <button onClick={() => onSelect(undefined)}>清除分组筛选</button> : null}</div>
    {!snapshot ? <p>识别完成后生成分组。已有识别结果可点“聚类已有结果”。</p> : <>
      <p>模型建议 · 待人工核对。点选分组在 Grid / Loupe 浏览。</p>
      <div className={styles.groups}>
        {snapshot.clusters.map((group, index) => <button key={group.id} className={selected?.clusterId === group.id && selected.snapshotId === snapshot.snapshotId ? styles.selected : ""}
          aria-pressed={selected?.clusterId === group.id && selected.snapshotId === snapshot.snapshotId}
          onClick={() => { onSelect({ snapshotId: snapshot.snapshotId, clusterId: group.id }); setMessage(""); }}>
          {group.cover ? <Cover asset={group.cover} /> : null}<strong>{group.people.map(p => p.displayName).filter(Boolean).join(" / ") || `匿名组 ${index + 1}`}<small>{group.people.length ? "已加入人物 · 成员仍需核对" : "匿名建议"}</small></strong><span>{group.members.length} 张照片</span>
        </button>)}
        {snapshot.ungrouped.length ? <button aria-pressed={selected?.clusterId === "ungrouped"} className={selected?.clusterId === "ungrouped" ? styles.selected : ""}
          onClick={() => onSelect({ snapshotId: snapshot.snapshotId, clusterId: "ungrouped" })}>未分组 / 歧义 · {snapshot.ungrouped.length} 个人脸</button> : null}
      </div>
      <p>未检测到人脸：{snapshot.noFaceCount} 张。缺失或失效证据：{snapshot.unavailableCount} 项。</p>
      {selected && selected.snapshotId !== snapshot.snapshotId ? <p role="status">分组已更新，请重新选择。</p> : null}
      {cluster ? <form className={styles.adopt} onSubmit={event => { event.preventDefault(); void adopt(); }}>
        <label>加入人物<select aria-label="分组加入人物" value={subject} onChange={event => setSubject(event.target.value)} disabled={busy}>
          <option value="">新建人物</option>{people.map((person, index) => <option key={person.id} value={person.id}>{person.displayName || `匿名人物 ${index + 1}`}</option>)}
        </select></label>
        {!subject ? <input aria-label="新人物名称" placeholder="姓名（可稍后填写）" value={name} onChange={event => setName(event.target.value)} disabled={busy} /> : null}
        <button type="submit" disabled={busy}>{busy ? "正在加入…" : "加入待确认"}</button>
        <small>逐图确认后可设置参考照片、确认身份或关联历史人物。已有“不属于”等决定会保留。</small>
      </form> : null}
    </>}
    {message ? <p role="status">{message}</p> : null}
    {failure || error ? <p role="alert">{failure || error}</p> : null}
  </section>;
}
