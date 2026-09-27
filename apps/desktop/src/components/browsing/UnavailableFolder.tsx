import * as Menu from "@radix-ui/react-dropdown-menu";
import { Folder, CircleAlert, LoaderCircle } from "lucide-react";
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
  const cancel = useRef<HTMLButtonElement>(null);
  const run = async (action: () => Promise<void>) => {
    setBusy(true); setFailure(undefined);
    try { await action(); } catch (cause) { setFailure(String(cause)); }
    finally { setBusy(false); }
  };
  const uncertain = plan?.entries.filter(entry => entry.status !== "verified").length ?? 0;
  return <div className={styles.row} title={error ? `${path}\n${error}` : path}>
    <Folder size={15} aria-hidden="true" />
    <div className={styles.name}><span>{path.replace(/[\\/]+$/, "").split(/[\\/]/).at(-1)}</span>
      <small>{restoring || busy ? t("folderRecovering") : t("folderUnavailable")}</small></div>
    {restoring ? <LoaderCircle size={14} className="is-spinning" /> : <Menu.Root>
      <Menu.Trigger ref={trigger} className={styles.trigger} disabled={busy} aria-label={`${t("folderRecoveryActions")}: ${path}`}><CircleAlert size={15} /></Menu.Trigger>
      <Menu.Portal><Menu.Content className={styles.menu} side="right" sideOffset={6} collisionPadding={8} onKeyDown={event => event.stopPropagation()}>
        <Menu.Item className={styles.item} onSelect={() => void run(async () => {
          const destination = await chooseFolder();
          if (destination) { setAcknowledged(false); setPlan(await planRootRelocation(path, destination)); }
        })}>{t("folderRelocate")}</Menu.Item>
        <Menu.Item className={styles.item} onSelect={() => void run(() => onRetry(path))}>{t("folderRetry")}</Menu.Item>
        <Menu.Separator className={styles.separator} />
        <Menu.Item className={styles.item} onSelect={() => { setFailure(undefined); setRemove(true); }}>{t("folderRemoveEntry")}</Menu.Item>
      </Menu.Content></Menu.Portal>
    </Menu.Root>}
    {failure && !plan && !remove ? <small className={styles.error} role="alert">{failure}</small> : null}
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
      title={t("folderRemoveEntry")} description={t("folderRemoveHint")} initialFocusRef={cancel} returnFocusRef={trigger}>
      <code className={styles.paths}>{path}</code>
      {failure ? <p role="alert" className={styles.error}>{failure}</p> : null}
      <div className={styles.actions}><Button ref={cancel} disabled={busy} onClick={() => setRemove(false)}>{t("cancel")}</Button>
        <Button variant="danger" loading={busy} onClick={() => void run(() => onRemove(path))}>{t("folderRemoveEntry")}</Button></div>
    </Dialog> : null}
  </div>;
}
