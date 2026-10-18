import { Download, FolderSearch, RefreshCw } from "lucide-react";
import type { MessageKey } from "@/lib/i18n";
import type { FolderMetadataSyncState } from "@/lib/hooks/useFolderMetadataSync";
import type { ExiftoolSetupAction } from "@/lib/projection/metadataProvider";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import styles from "./FolderMetadataSyncDialog.module.css";

interface Props {
  state: FolderMetadataSyncState;
  onSetup: (source: ExiftoolSetupAction) => void;
  onCancel: () => void;
  onClose: () => void;
  t: (key: MessageKey) => string;
}

export function FolderMetadataSyncDialog({ state, onSetup, onCancel, onClose, t }: Props) {
  const done = state.phase === "done";
  const summary = t("folderMetadataSyncSummary")
    .replace("{success}", String(state.succeeded))
    .replace("{unchanged}", String(state.unchanged))
    .replace("{failed}", String(state.failureCount))
    .replace("{skipped}", String(state.skipped));
  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}
    title={t("folderMetadataSync")} description={t("folderMetadataSyncScope")}
    dismissOnOutsideClick={false}>
    <p className={styles.path}>{state.directory}</p>
    {state.phase === "setup" ? <>
      <p className={styles.description}>{t("exiftoolRequiredBody")}</p>
      <p className={styles.description}>{t("exiftoolManualInstallHint")}</p>
      <div className={styles.actions}>
        <Button variant="primary" disabled={state.settingUp}
          loading={state.settingUp && state.setupAction === "download"} onClick={() => onSetup("download")}>
          <Download size={13} />{t("downloadExiftool")}
        </Button>
        <Button disabled={state.settingUp} onClick={() => onSetup("select")}>
          <FolderSearch size={13} />{t("selectExiftool")}
        </Button>
        <Button disabled={state.settingUp} loading={state.settingUp && state.setupAction === "check"}
          onClick={() => onSetup("check")}>
          <RefreshCw size={13} />{t("recheckExiftool")}
        </Button>
      </div>
    </> : <div role="status" aria-live="polite" className={styles.status}>
      {state.phase === "scanning" ? t("folderMetadataSyncScanning")
        : done ? t(state.error ? "folderMetadataSyncFailed"
          : state.cancelled ? "folderMetadataSyncCancelled" : "folderMetadataSyncDone")
        : `${t("syncingEmbeddedMetadata")} ${state.completed} / ${state.total}`}
      {state.phase === "writing" ? <progress max={state.total} value={state.completed} /> : null}
      <p>{summary}</p>
    </div>}
    {state.error ? <p role="alert" className={styles.error}>{state.error}</p> : null}
    {state.failures.length ? <details className={styles.failures}>
      <summary>{t("folderMetadataSyncFailures")}</summary>
      <ul>{state.failures.map(failure => <li key={failure.path}>
        <strong>{failure.path}</strong><span>{failure.error}</span>
        {failure.uncertain ? <span>{t("folderMetadataSyncUncertain")}</span> : null}
      </li>)}</ul>
    </details> : null}
    <div className={styles.actions}>
      <Button onClick={done ? onClose : onCancel} disabled={!done && state.cancelled}>
        {t(done ? "folderMetadataSyncClose" : state.cancelled ? "folderMetadataSyncStopping" : "cancel")}
      </Button>
    </div>
  </Dialog>;
}
