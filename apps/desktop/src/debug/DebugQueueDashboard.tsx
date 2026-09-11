import { Activity, CirclePause, CirclePlay, RefreshCw, Server, Waves } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { DebugQueueItem, DebugQueueSnapshot, DebugQueueState } from "../types";
import { getDebugQueueSnapshot, isTauri } from "../lib/api";
import {
  aggregatePreviewDebugSnapshot,
  getPreviewDebugSnapshot,
  type TrackedPreview,
} from "../lib/previewDebug";
import "./debugQueueDashboard.css";

const REFRESH_MS = 250;
const PRIORITY_ORDER = ["loupe", "selected", "active", "visible", "filter", "session", "nearby", "background", "preload"];

function basename(path?: string) {
  return path?.split(/[\\/]/).pop() || "—";
}

function priorityIndex(priority: string) {
  const index = PRIORITY_ORDER.indexOf(priority);
  return index < 0 ? PRIORITY_ORDER.length : index;
}

function queueItemFromWeb(item: TrackedPreview): DebugQueueItem {
  return {
    key: String(item.id),
    stage: item.resourceLabel ? `${item.stage} · ${item.resourceLabel}` : item.stage,
    priority: item.priority,
    path: item.assetName,
    resource: item.resourceKey?.replace(/^[^:]+:/, ""),
    consumers: item.consumers ?? 1,
  };
}

function QueueRows({ items, empty }: { items: DebugQueueItem[]; empty: string }) {
  if (!items.length) return <div className="debug-empty">{empty}</div>;
  return (
    <div className="debug-rows">
      {items.map((item) => (
        <div className="debug-row" key={item.key}>
          <span className={`debug-priority priority-${item.priority}`}>{item.priority}</span>
          <span className="debug-file" title={item.path}>
            <b>{basename(item.path)}</b>
            {item.rootPath ? <small title={item.rootPath}>root · {item.rootPath}</small> : null}
            {!item.rootPath && item.resource ? <small>{item.resource}</small> : null}
          </span>
          <span className="debug-stage">
            <span>{item.stage}</span>
            {item.pendingCount !== undefined ? (
              <small>
                pending {item.pendingCount.toLocaleString()} · assets {(item.assetCount ?? 0).toLocaleString()} · dirs {(item.directoryCount ?? 0).toLocaleString()}
              </small>
            ) : null}
          </span>
          <span className="debug-consumers">×{item.consumers}</span>
        </div>
      ))}
    </div>
  );
}

function QueueCard({ queue }: { queue: DebugQueueState }) {
  const pending = useMemo(
    () => [...queue.pending].sort((a, b) => priorityIndex(a.priority) - priorityIndex(b.priority) || (a.rank ?? 0) - (b.rank ?? 0)),
    [queue.pending],
  );
  return (
    <section className="debug-card">
      <header>
        <div>
          <span className="debug-kicker">NATIVE SCHEDULER</span>
          <h2>{queue.name}</h2>
        </div>
        <div className="debug-capacity"><Waves size={14} /> {queue.active.length}/{queue.concurrency}</div>
      </header>
      <div className="debug-lane debug-active-lane">
        <h3><CirclePlay size={15} /> RUNNING <b>{queue.active.length}</b></h3>
        <QueueRows items={queue.active} empty="No active work" />
      </div>
      <div className="debug-lane">
        <h3><CirclePause size={15} /> PENDING <b>{pending.length}</b></h3>
        <QueueRows items={pending} empty="Queue is clear" />
      </div>
    </section>
  );
}

export function DebugQueueDashboard({ onClose }: { onClose: () => void }) {
  const [snapshot, setSnapshot] = useState<DebugQueueSnapshot>();
  const [webQueue, setWebQueue] = useState<DebugQueueState>({ name: "webviewRequest", concurrency: 0, pending: [], active: [] });
  const [paused, setPaused] = useState(false);
  const [pageActive, setPageActive] = useState(() => !isTauri());
  const [error, setError] = useState<string>();
  const refreshing = useRef(false);

  const refresh = useCallback(async () => {
    if (refreshing.current) return;
    refreshing.current = true;
    const local = aggregatePreviewDebugSnapshot(getPreviewDebugSnapshot());
    setWebQueue({
      name: "webviewRequest",
      concurrency: local.loading.length,
      pending: local.waiting.map(queueItemFromWeb),
      active: local.loading.map(queueItemFromWeb),
    });
    try {
      setSnapshot(await getDebugQueueSnapshot());
      setError(undefined);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      refreshing.current = false;
    }
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let stopListening: (() => void) | undefined;
    void getCurrentWindow().isVisible().then((visible) => {
      if (!disposed) setPageActive(visible);
    });
    void listen<boolean>("debug-queue-visibility", (event) => {
      setPageActive(event.payload);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else stopListening = unlisten;
    });
    return () => {
      disposed = true;
      stopListening?.();
    };
  }, []);

  useEffect(() => {
    if (paused || !pageActive) return;
    void refresh();
    const timer = window.setInterval(() => void refresh(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [pageActive, paused, refresh]);

  const queues = [webQueue, ...(snapshot?.queues ?? [])];
  const pending = queues.reduce((total, queue) => total + queue.pending.length, 0);
  const active = queues.reduce((total, queue) => total + queue.active.length, 0);

  return (
    <main className="debug-dashboard">
      <div className="debug-topbar">
        <div className="debug-title">
          <div className="debug-logo"><Activity size={22} /></div>
          <div><span>OXYVIEWER / DEBUG</span><h1>Queue Observatory</h1></div>
        </div>
        <div className="debug-actions">
          <span className="debug-live"><i className={paused ? "paused" : ""} />{paused ? "PAUSED" : "LIVE · 4 Hz"}</span>
          <button onClick={() => setPaused((value) => !value)}>{paused ? <CirclePlay size={16} /> : <CirclePause size={16} />}{paused ? "Resume" : "Pause"}</button>
          <button onClick={() => void refresh()} disabled={!paused}><RefreshCw size={16} /> Refresh</button>
          <button onClick={onClose}>Back to app</button>
        </div>
      </div>

      <div className="debug-summary">
        <div><span>ACTIVE</span><strong>{active}</strong></div>
        <div><span>PENDING</span><strong>{pending}</strong></div>
        <div><span>SCHEDULERS</span><strong>{queues.length}</strong></div>
        <div><span>LAST SAMPLE</span><strong className="debug-time">{snapshot ? new Date(snapshot.capturedAtUnixMs).toLocaleTimeString() : "—"}</strong></div>
      </div>

      {error && <div className="debug-error"><Server size={16} /> Native snapshot unavailable: {error}</div>}
      {!!snapshot?.staleQueues.length && <div className="debug-error">Busy queues: {snapshot.staleQueues.join(", ")}. Showing the last available sample.</div>}
      <div className="debug-grid">{queues.map((queue) => <QueueCard queue={queue} key={queue.name} />)}</div>
      <footer>Read-only diagnostics · polling begins only while this page is open · debug builds and explicit performance runs only</footer>
    </main>
  );
}
