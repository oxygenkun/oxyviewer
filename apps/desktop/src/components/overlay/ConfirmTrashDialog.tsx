import { Trash2 } from "lucide-react";
import { useRef } from "react";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import styles from "./ConfirmTrashDialog.module.css";
import type { MessageKey } from "@/lib/i18n";
import type { FileDeletionMode } from "@/types";

interface ConfirmTrashDialogProps {
  deletionMode: FileDeletionMode;
  itemName: string;
  itemCount?: number;
  onCancel: () => void;
  onConfirm: () => void;
  t: (key: MessageKey) => string;
}

export function ConfirmTrashDialog({
  deletionMode,
  itemName,
  itemCount = 1,
  onCancel,
  onConfirm,
  t,
}: ConfirmTrashDialogProps) {
  const permanent = deletionMode === "permanent";
  const body = itemCount > 1
    ? t(permanent ? "permanentDeleteConfirmMultipleBody" : "trashConfirmMultipleBody")
      .replace("{count}", String(itemCount))
    : t(permanent ? "permanentDeleteConfirmBody" : "trashConfirmBody").replace("{name}", itemName);
  const cancelRef = useRef<HTMLButtonElement>(null);
  return <Dialog open onOpenChange={open => { if (!open) onCancel(); }}
    role="alertdialog" dismissOnOutsideClick initialFocusRef={cancelRef}
    title={<span className={styles.title}><Trash2 size={18} aria-hidden="true" />{t(permanent ? "permanentDeleteConfirmTitle" : "trashConfirmTitle")}</span>}
    description={body}>
    <div className={styles.actions}>
      <Button ref={cancelRef} onClick={onCancel}>{t("cancel")}</Button>
      <Button variant="danger" onClick={onConfirm}>
        {t(permanent ? "confirmPermanentDelete" : "confirmTrash")}
      </Button>
    </div>
  </Dialog>;
}
