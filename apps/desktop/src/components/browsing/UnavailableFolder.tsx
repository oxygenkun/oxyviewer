import * as Menu from "@radix-ui/react-dropdown-menu";
import { FolderX, Link2, LoaderCircle, X } from "lucide-react";
import { useRef, useState } from "react";
import { chooseFolder, planRootRelocation } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type { RootRelocationPlan } from "@/types";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import styles from "./UnavailableFolder.module.css";

interface Props {
  path: string;
  error?: string;
  restoring?: boolean;
  onRetry: (path: string) => Promise<void>;
  onRemove: (path: string) => Promise<void>;
  onRelocate: (plan: RootRelocationPlan) => Promise<void>;
  t: (key: MessageKey) => string;
}

export function UnavailableFolder({ path, error, restoring, onRetry, onRemove, onRelocate, t }: Props) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string>();
  const [plan, setPlan] = useState<RootRelocationPlan>();
  const [remove, setRemove] = useState(false);
  const [acknowledged, setAcknowledged] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const removeTrigger = useRef<HTMLButtonElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const run = async (action: () => Promise<void>) => {
    setBusy(true); setFailure(undefined);
    try { await action(); } catch (cause) { setFailure(String(cause)); }
    finally { setBusy(false); }
  };
  const uncertain = plan?.entries.filter(entry => entry.status !== "verified").length ?? 0;
  const name = path.replace(/[\\/]+$/, "").split(/[\\/]/).at(-1);
  const status = restoring || busy ? t("folderRecovering") : t("folderUnavailable");
  return <div className={`tree-row tree-row--directory ${styles.row}`} title={[path, status, failure ?? error].filter(Boolean).join("\n")}>
    <span className="tree-row__toggle" aria-hidden="true" />
    <div className={`tree-row__main ${styles.name}`}><FolderX size={15} aria-hidden="true" /><span>{name}</span></div>
    <span className={styles.srOnly}>{status}</span>
    <Menu.Root>
      <Menu.Trigger ref={trigger} className={styles.trigger} disabled={busy || restoring}
        title={t("folderRecoveryActions")} aria-label={`${t("folderRecoveryActions")}: ${path}`}>
        {restoring || busy ? <LoaderCircle size={13} className="tree-row__loader" /> : <Link2 size={14} />}
      </Menu.Trigger>
      <Menu.Portal><Menu.Content className={styles.menu} side="right" sideOffset={6} collisionPadding={8} onKeyDown={event => event.stopPropagation()}>
        <Menu.Item className={styles.item} onSelect={() => void run(async () => {
          const destination = await chooseFolder();
          if (destination) { setAcknowledged(false); setPlan(await planRootRelocation(path, destination)); }
        })}>{t("folderRelocate")}</Menu.Item>
        <Menu.Item className={styles.item} onSelect={() => void run(() => onRetry(path))}>{t("folderRetry")}</Menu.Item>
      </Menu.Content></Menu.Portal>
    </Menu.Root>
    <button ref={removeTrigger} className="tree-row__remove" disabled={busy || restoring}
      title={t("folderRemoveEntry")} aria-label={`${t("folderRemoveEntry")}: ${name}`}
      onClick={() => { setFailure(undefined); setRemove(true); }}><X size={12} /></button>
    {failure && !plan && !remove ? <span className={styles.srOnly} role="alert">{failure}</span> : null}
    {plan ? <Dialog open dismissOnOutsideClick={!busy} onOpenChange={open => { if (!open && !busy) setPlan(undefined); }}
      title={t("folderRelocateTitle")} description={t("folderRelocateHint")} initialFocusRef={cancel} returnFocusRef={trigger}>
      <div className={styles.paths}><code>{plan.oldRoot}</code><span>↓</span><code>{plan.newRoot}</code></div>
      <div className={styles.summary}>{t("folderVerified")}: {plan.entries.length - uncertain} · {t("folderUnverified")}: {uncertain}</div>
      <ul className={styles.entries}>{plan.entries.map(entry => <li key={entry.oldPath}>
        <span>{entry.oldPath.slice(plan.oldRoot.length + 1)}</span>
        <small>{t(entry.status === "verified" ? "folderVerified" : entry.status === "missing" ? "folderMissing" : "folderUnverified")}</small>
      </li>)}</ul>
      {uncertain ? <label className={styles.acknowledge}><input type="checkbox" checked={acknowledged} disabled={busy} onChange={event => setAcknowledged(event.target.checked)} />{t("folderRelocateAcknowledge")}</label> : null}
      {failure ? <p role="alert" className={styles.error}>{failure}</p> : null}
      <div className={styles.actions}><Button ref={cancel} disabled={busy} onClick={() => setPlan(undefined)}>{t("cancel")}</Button>
        <Button variant="primary" loading={busy} disabled={!!uncertain && !acknowledged} onClick={() => void run(async () => { await onRelocate(plan); setPlan(undefined); })}>{t("folderRelocateConfirm")}</Button></div>
    </Dialog> : null}
    {remove ? <Dialog open dismissOnOutsideClick={!busy} onOpenChange={open => { if (!open && !busy) setRemove(false); }}
      title={t("folderRemoveEntry")} description={t("folderRemoveHint")} initialFocusRef={cancel} returnFocusRef={removeTrigger}>
      <code className={styles.paths}>{path}</code>
      {failure ? <p role="alert" className={styles.error}>{failure}</p> : null}
      <div className={styles.actions}><Button ref={cancel} disabled={busy} onClick={() => setRemove(false)}>{t("cancel")}</Button>
        <Button variant="danger" loading={busy} onClick={() => void run(() => onRemove(path))}>{t("folderRemoveEntry")}</Button></div>
    </Dialog> : null}
  </div>;
}
