import { Trash2 } from "lucide-react";
import { useEffect } from "react";
import { createPortal } from "react-dom";
import type { MessageKey } from "../lib/i18n";
import type { FileDeletionMode } from "../types";

interface ConfirmTrashDialogProps {
  deletionMode: FileDeletionMode;
  itemName: string;
  onCancel: () => void;
  onConfirm: () => void;
  t: (key: MessageKey) => string;
}

export function ConfirmTrashDialog({
  deletionMode,
  itemName,
  onCancel,
  onConfirm,
  t,
}: ConfirmTrashDialogProps) {
  const permanent = deletionMode === "permanent";
  useEffect(() => {
    const cancelOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", cancelOnEscape);
    return () => document.removeEventListener("keydown", cancelOnEscape);
  }, [onCancel]);

  return createPortal(
    <div className="trash-confirm-overlay" onClick={onCancel}>
      <div
        aria-labelledby="trash-confirm-title"
        aria-describedby="trash-confirm-body"
        aria-modal="true"
        className="trash-confirm-dialog"
        onClick={(event) => event.stopPropagation()}
        role="alertdialog"
      >
        <span className="trash-confirm-dialog__icon"><Trash2 size={18} /></span>
        <div>
          <strong id="trash-confirm-title">
            {t(permanent ? "permanentDeleteConfirmTitle" : "trashConfirmTitle")}
          </strong>
          <p id="trash-confirm-body">
            {t(permanent ? "permanentDeleteConfirmBody" : "trashConfirmBody").replace("{name}", itemName)}
          </p>
        </div>
        <div className="trash-confirm-dialog__actions">
          <button autoFocus onClick={onCancel}>{t("cancel")}</button>
          <button className="is-danger" onClick={onConfirm}>
            {t(permanent ? "confirmPermanentDelete" : "confirmTrash")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
