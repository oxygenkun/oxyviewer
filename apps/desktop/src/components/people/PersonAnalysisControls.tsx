import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { startPersonClustering, cancelPersonOperation, downloadPersonModel, getPersonModels, getPersonOperation, importPersonModel, startFolderPersonAnalysis } from "@/lib/api";
import styles from "./PersonAnalysisControls.module.css";

export function PersonAnalysisControls({ sessionId, folderPath, modelsOnly = false }: { sessionId?: string; folderPath?: string; modelsOnly?: boolean }) {
  const client = useQueryClient();
  const [manage, setManage] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string>();
  const [cancelRequested, setCancelRequested] = useState(false);
  const models = useQuery({ queryKey: ["person-models"], queryFn: getPersonModels, staleTime: Infinity });
  const operation = useQuery({ queryKey: ["person-operation"], queryFn: getPersonOperation, refetchInterval: 1000 });
  const status = operation.data;
  const active = Boolean(status && ["downloading", "preparing", "analysing", "clustering"].includes(status.state));
  const ready = Boolean(models.data?.length && models.data.every(model => model.installed));
  const finished = useRef("");
  useEffect(() => {
    if (!status || active || finished.current === status.operationId) return;
    finished.current = status.operationId;
    setCancelRequested(false);
    void client.invalidateQueries({ queryKey: ["person-models"] });
    void client.invalidateQueries({ queryKey: ["person-detections"] });
    void client.invalidateQueries({ queryKey: ["person-clusters"] });
    void client.invalidateQueries({ queryKey: ["assets"] });
    void client.invalidateQueries({ queryKey: ["people-workspace"] });
    void client.invalidateQueries({ queryKey: ["global-people"] });
  }, [status, active, client]);
  const perform = async (action: () => Promise<unknown>) => {
    setSubmitting(true); setError(undefined);
    try { await action(); await operation.refetch(); }
    catch (cause) { setError(String(cause)); }
    finally { setSubmitting(false); }
  };
  return <section className={styles.panel} aria-label="人物识别">
    <div className={styles.actions}>
      <button onClick={() => setManage(value => !value)} aria-expanded={manage}>管理模型</button>
      {!modelsOnly ? <><button disabled={!sessionId || !folderPath || !ready || active || submitting}
        onClick={() => void perform(() => startFolderPersonAnalysis(sessionId!, folderPath!))}>识别选中文件夹</button>
      <button disabled={!sessionId || !folderPath || active || submitting} onClick={() => void perform(() => startPersonClustering(sessionId!, folderPath!))}>聚类已有结果</button></> : null}
    </div>
    {!modelsOnly ? !folderPath ? <p>先在收藏夹中选择文件夹。</p> : <p title={folderPath}>当前文件夹：{folderPath.split(/[\\/]/).filter(Boolean).at(-1)}</p> : null}
    {!modelsOnly ? <p>检测人脸、提取特征并生成匿名分组，不含子文件夹。分组需人工核对；已有特征可直接聚类。</p> : null}
    {!ready && !models.isLoading && (!modelsOnly || manage) ? <p>识别前请在“管理模型”中准备所需模型。</p> : null}
    {manage ? <div className={styles.models}>
      {models.isLoading ? <p>正在检查模型…</p> : models.data?.map(model => <div key={model.id}>
        <strong>{model.name}</strong><span> · {model.installed ? "已安装" : "未安装"}</span>
        {model.sizeBytes ? <small>{(model.sizeBytes / 1048576).toFixed(1)} MB</small> : null}
        <p>{model.usage}</p>
        {model.sourceUrl ? <a href={model.sourceUrl} target="_blank" rel="noreferrer">模型来源与说明</a> : null}
        {model.error ? <p role="alert">{model.error}</p> : null}
        {!model.installed ? <div className={styles.actions}>
          <button disabled={!model.downloadAvailable || active || submitting} onClick={() => void perform(() => downloadPersonModel(model.id))}>下载</button>
          {model.id !== "runtime" && model.id !== "demo" ? <button disabled={active || submitting} onClick={() => void perform(() => importPersonModel(model.id))}>导入 ONNX</button> : null}
        </div> : null}
      </div>)}
    </div> : null}
    {status ? <div aria-live="polite">
      {status.folderPath ? <p title={status.folderPath}>任务文件夹：{status.folderPath.split(/[\\/]/).filter(Boolean).at(-1)}</p> : null}
      <p>{status.detail}</p>
      {active ? <progress max={status.total || 1} value={status.total ? status.completed : undefined} /> : null}
      {status.run ? <p>已完成 {status.run.completedTasks} / {status.run.totalTasks} 个步骤；失败 {status.run.failedTasks} 个</p> : null}
      {status.error ? <p role="alert">{status.error}</p> : null}
      {active ? <button disabled={cancelRequested || submitting} onClick={() => void perform(async () => { await cancelPersonOperation(status.operationId); setCancelRequested(true); })}>{cancelRequested ? "正在取消…" : "取消任务"}</button> : null}
    </div> : null}
    {error || models.error || operation.error ? <p role="alert">{error ?? String(models.error ?? operation.error)}</p> : null}
  </section>;
}
